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
mod tests {
    use std::time::Duration;

    use async_nats::jetstream::{self};
    use bytes::Bytes;
    use testcontainers_modules::nats::{Nats, NatsServerCmd};
    use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};

    use super::*;
    use crate::processor::execution::worker::dispatcher::DeliveredMessage;

    struct NatsServer {
        _container: ContainerAsync<Nats>,
        url: String,
    }

    impl NatsServer {
        async fn start() -> Self {
            let cmd = NatsServerCmd::default().with_jetstream();
            let container = Nats::default()
                .with_cmd(&cmd)
                .start()
                .await
                .expect("start NATS testcontainer for JetStream unit tests");
            let host = container.get_host().await.expect("get NATS testcontainer host");
            let port = container
                .get_host_port_ipv4(4222)
                .await
                .expect("get NATS testcontainer port");
            let url = format!("{host}:{port}");
            Self {
                _container: container,
                url,
            }
        }

        fn url(&self) -> &str {
            &self.url
        }
    }

    fn js_ack_reply(delivered: u64) -> String {
        format!("$JS.ACK._.hash.STREAM.consumer.{delivered}.10.1.1704000000000000000.0.tok")
    }

    async fn jetstream_message(delivered: u64) -> (NatsServer, jetstream::Message) {
        let server = NatsServer::start().await;
        let client = async_nats::ConnectOptions::new()
            .connection_timeout(Duration::from_secs(2))
            .connect(server.url())
            .await
            .expect("connect to NATS testcontainer");
        let context = jetstream::new(client);
        let message = jetstream::Message {
            message: async_nats::Message {
                subject: "scheduler.test".into(),
                reply: Some(js_ack_reply(delivered).into()),
                payload: Bytes::new(),
                headers: None,
                status: None,
                description: None,
                length: 0,
            },
            context,
        };
        (server, message)
    }

    #[test]
    fn retry_delay_backs_off_exponentially_and_caps() {
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(2));
        assert_eq!(retry_delay(3), Duration::from_secs(4));
        assert_eq!(retry_delay(6), Duration::from_secs(30));
        assert_eq!(retry_delay(i64::MAX), Duration::from_secs(30));
    }

    fn execution_stream(allow_message_schedules: bool) -> jetstream::stream::Config {
        jetstream::stream::Config {
            name: SCHEDULE_EXECUTION_STREAM.to_string(),
            allow_message_schedules,
            ..Default::default()
        }
    }

    #[test]
    fn scheduling_support_requires_nats_2_14_or_newer() {
        let stream = execution_stream(true);

        assert!(verify_message_scheduling_support("2.14.0", &stream).is_ok());
        assert!(verify_message_scheduling_support("2.15.1-beta.2", &stream).is_ok());
        assert!(verify_message_scheduling_support("3.0.0", &stream).is_ok());

        // 2.12/2.13 only deliver `@at` schedules; cron and `@every` would
        // publish successfully and never fire.
        assert!(matches!(
            verify_message_scheduling_support("2.12.0", &stream),
            Err(SchedulingSupportError::ServerTooOld { version }) if version == "2.12.0"
        ));
        assert!(matches!(
            verify_message_scheduling_support("2.13.1", &stream),
            Err(SchedulingSupportError::ServerTooOld { .. })
        ));
        assert!(matches!(
            verify_message_scheduling_support("1.4.0", &stream),
            Err(SchedulingSupportError::ServerTooOld { .. })
        ));
    }

    #[test]
    fn scheduling_support_rejects_unrecognized_server_versions() {
        let stream = execution_stream(true);

        for version in ["", "nats", "2", "v2.12.0"] {
            assert!(matches!(
                verify_message_scheduling_support(version, &stream),
                Err(SchedulingSupportError::UnrecognizedServerVersion { .. })
            ));
        }
    }

    #[test]
    fn scheduling_support_requires_allow_message_schedules_on_the_stream() {
        let error = verify_message_scheduling_support("2.14.0", &execution_stream(false)).unwrap_err();

        assert!(matches!(
            error,
            SchedulingSupportError::SchedulesNotAllowed { ref stream } if stream == SCHEDULE_EXECUTION_STREAM
        ));
        assert!(error.to_string().contains("allow_message_schedules"));
    }

    #[test]
    fn event_filter_is_derived_from_the_event_subject_prefix() {
        use crate::processor::execution::reconciliation::EVENT_SUBJECT_PREFIX;

        assert_eq!(SCHEDULE_EVENT_FILTER, format!("{EVENT_SUBJECT_PREFIX}.>"));
    }

    #[test]
    fn consumer_config_matches_the_scheduler_contract() {
        let config = scheduler_execution_consumer_config();

        assert_eq!(config.durable_name.as_deref(), Some("scheduler_execution_v1"));
        assert!(matches!(config.deliver_policy, DeliverPolicy::All));
        assert!(matches!(config.ack_policy, AckPolicy::Explicit));
        assert!(matches!(config.replay_policy, ReplayPolicy::Instant));
        assert_eq!(config.ack_wait, Duration::from_secs(120));
        assert_eq!(config.max_deliver, -1);
        assert_eq!(config.filter_subject, "scheduler.schedules.events.v1.>");
        // max_ack_pending bounds concurrency but is intentionally not 1: global
        // serialization is not wanted.
        assert_eq!(config.max_ack_pending, 256);
        assert_ne!(config.max_ack_pending, 1);
        assert!(config.backoff.is_empty());
    }

    #[tokio::test]
    async fn jetstream_delivered_message_reports_redelivery() {
        let (_first_server, first_message) = jetstream_message(1).await;
        let (_again_server, again_message) = jetstream_message(2).await;
        let first = JetStreamDeliveredMessage::new(first_message);
        let again = JetStreamDeliveredMessage::new(again_message);

        assert!(!DeliveredMessage::is_redelivery(&first));
        assert!(DeliveredMessage::is_redelivery(&again));
    }

    #[tokio::test]
    async fn jetstream_delivered_message_settlement_methods_run() {
        let (_server, message) = jetstream_message(1).await;
        let delivered = JetStreamDeliveredMessage::new(message);
        DeliveredMessage::ack(&delivered).await.unwrap();
        DeliveredMessage::term(&delivered).await.unwrap();
        DeliveredMessage::retry(&delivered).await.unwrap();

        let inner = delivered.into_inner();
        assert_eq!(inner.subject.as_str(), "scheduler.test");
    }

    #[tokio::test]
    async fn jetstream_delivered_message_new_and_into_inner_round_trip() {
        let (_server, message) = jetstream_message(1).await;
        let wrapped = JetStreamDeliveredMessage::new(message);
        let inner = wrapped.into_inner();
        assert_eq!(inner.subject.as_str(), "scheduler.test");
    }
}
