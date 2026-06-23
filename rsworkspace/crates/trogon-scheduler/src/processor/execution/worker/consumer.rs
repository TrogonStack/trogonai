//! Durable pull-consumer configuration and the scheduler's concrete NATS
//! naming.
//!
//! The scheduler component does not own the runtime durable consumer's
//! provisioning, but it defines the contract that consumer must satisfy. A new
//! durable replays all retained schedule events (`DeliverPolicy::All`) so the
//! processor can rebuild execution schedules from history; a normal restart
//! resumes from the durable's stored ack position and
//! `last_applied_stream_position` collapses any redelivered overlap into
//! duplicate/stale no-ops.
//!
//! Per-`ScheduleKey` ordering is enforced only within one process (the
//! dispatcher lanes). JetStream distributes pulled messages across consumer
//! instances arbitrarily, so concurrent instances can interleave side effects
//! for the same schedule: an instance still applying an older Paused can purge
//! the execution message a newer Resumed just published on another instance,
//! then resolve its CAS conflict as duplicate/stale — checkpoint says
//! Scheduled, subject is empty. Run a single active instance of this durable
//! until cross-instance per-key serialization exists.

use std::time::Duration;

use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};
use async_nats::jetstream::{self, AckKind, stream};

use super::dispatcher::DeliveredMessage;

/// Event stream the scheduler consumes persisted schedule events from.
pub const SCHEDULE_EVENT_STREAM: &str = "SCHEDULER_SCHEDULE_EVENTS";
/// Subject the scheduler consumer filters on. Keyed by `ScheduleKey`, never the
/// raw `ScheduleId`. Must stay `{EVENT_SUBJECT_PREFIX}.>`; a test enforces the
/// derivation since consts cannot be concatenated at compile time.
pub const SCHEDULE_EVENT_FILTER: &str = "scheduler.schedules.events.v1.>";
/// Durable name the scheduler pull consumer registers under.
pub const SCHEDULE_EVENT_CONSUMER: &str = "scheduler_execution_v1";
/// KV bucket holding the rebuildable scheduler checkpoint cache.
pub const SCHEDULE_STATE_BUCKET: &str = "SCHEDULER_SCHEDULE_STATE";
/// Execution schedule stream (must be provisioned with `AllowMsgSchedules`).
pub const SCHEDULE_EXECUTION_STREAM: &str = "SCHEDULER_SCHEDULE_EXECUTION";

/// The execution stream cannot honor `Nats-Schedule*` headers; publishes
/// would succeed but the scheduled messages would never fire.
#[derive(Debug, thiserror::Error)]
pub enum SchedulingSupportError {
    /// The full schedule feature set this processor publishes (cron, `@every`,
    /// schedule TTL, subject sampling) requires NATS server 2.14 or newer;
    /// 2.12 only delivers single `@at` schedules.
    #[error("NATS server {version} does not support cron/@every message schedules; 2.14.0 or newer is required")]
    ServerTooOld { version: String },
    /// The reported server version could not be parsed, so support cannot be
    /// verified.
    #[error("NATS server version '{version}' is not recognized; message schedule support cannot be verified")]
    UnrecognizedServerVersion { version: String },
    /// The execution stream is missing `allow_message_schedules: true`.
    #[error("stream '{stream}' is not provisioned with allow_message_schedules; scheduled publishes would never fire")]
    SchedulesNotAllowed { stream: String },
}

/// Verifies at startup that scheduled publishes can actually fire: the server
/// must be NATS >= 2.14 (2.12 introduced `@at` only; cron, `@every`, schedule
/// TTL, and subject sampling — all published by this processor — landed in
/// 2.14) and the execution stream must be provisioned with
/// `allow_message_schedules: true`. On an older server or a stream without the
/// flag, `Nats-Schedule*` publishes succeed silently and nothing ever fires.
///
/// Call with `client.server_info().version` and the execution stream's config
/// (e.g. `stream.cached_info().config`).
pub fn verify_message_scheduling_support(
    server_version: &str,
    execution_stream: &stream::Config,
) -> Result<(), SchedulingSupportError> {
    let mut parts = server_version.split('.').map(|part| {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u64>().ok()
    });
    let (Some(Some(major)), Some(Some(minor))) = (parts.next(), parts.next()) else {
        return Err(SchedulingSupportError::UnrecognizedServerVersion {
            version: server_version.to_string(),
        });
    };
    if (major, minor) < (2, 14) {
        return Err(SchedulingSupportError::ServerTooOld {
            version: server_version.to_string(),
        });
    }
    if !execution_stream.allow_message_schedules {
        return Err(SchedulingSupportError::SchedulesNotAllowed {
            stream: execution_stream.name.clone(),
        });
    }
    Ok(())
}

/// Builds the scheduler's durable pull-consumer configuration.
pub fn scheduler_execution_consumer_config() -> pull::Config {
    pull::Config {
        durable_name: Some(SCHEDULE_EVENT_CONSUMER.to_string()),
        description: Some("applies persisted schedule events to execution schedules".to_string()),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        ack_wait: Duration::from_secs(120),
        max_deliver: -1,
        filter_subject: SCHEDULE_EVENT_FILTER.to_string(),
        replay_policy: ReplayPolicy::Instant,
        max_waiting: 32,
        max_ack_pending: 256,
        max_batch: 64,
        max_expires: Duration::from_secs(5),
        backoff: Vec::new(),
        ..Default::default()
    }
}

/// JetStream pull message wired into [`spawn_dispatcher`](super::spawn_dispatcher).
pub struct JetStreamDeliveredMessage {
    message: jetstream::Message,
}

impl JetStreamDeliveredMessage {
    /// Wraps a consumer-delivered JetStream message for lane dispatch.
    pub fn new(message: jetstream::Message) -> Self {
        Self { message }
    }

    /// Returns the underlying JetStream message.
    pub fn into_inner(self) -> jetstream::Message {
        self.message
    }
}

/// Exponential redelivery delay derived from the delivery count. An explicit
/// `Nak(None)` redelivers immediately regardless of the consumer's `backoff`
/// configuration, so the delay must be supplied client-side or transient
/// failures hot-loop against the backend for the whole outage window.
fn retry_delay(delivered: i64) -> Duration {
    const BASE: Duration = Duration::from_secs(1);
    const MAX: Duration = Duration::from_secs(30);
    let attempts = u32::try_from(delivered.max(1) - 1).unwrap_or(u32::MAX).min(8);
    BASE.saturating_mul(1u32 << attempts).min(MAX)
}

impl DeliveredMessage for JetStreamDeliveredMessage {
    fn is_redelivery(&self) -> bool {
        self.message.info().is_ok_and(|info| info.delivered > 1)
    }

    /// An unparseable ACK reply reports `i64::MAX`, not 1: a fallback of 1
    /// would pin the count below every delivery ceiling and the record would
    /// NAK-loop forever instead of being poisoned with a durable failure.
    fn delivery_count(&self) -> i64 {
        self.message.info().map_or(i64::MAX, |info| info.delivered)
    }

    async fn ack(&self) -> Result<(), String> {
        self.message
            .ack()
            .await
            .map_err(|err| format!("jetstream ack failed: {err}"))
    }

    async fn term(&self) -> Result<(), String> {
        self.message
            .ack_with(AckKind::Term)
            .await
            .map_err(|err| format!("jetstream term failed: {err}"))
    }

    async fn retry(&self) -> Result<(), String> {
        let delay = retry_delay(DeliveredMessage::delivery_count(self));
        self.message
            .ack_with(AckKind::Nak(Some(delay)))
            .await
            .map_err(|err| format!("jetstream nak failed: {err}"))
    }
}

#[cfg(test)]
mod tests;
