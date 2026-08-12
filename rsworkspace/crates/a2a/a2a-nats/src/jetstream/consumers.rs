//! JetStream consumer configs for A2A task event delivery.
//!
//! Task event subjects are scoped to the task, not the request (ADR#0055), so a
//! filter narrows delivery by `task_id` and concurrent subscribers of the same
//! task are told apart in process by the `Trogon-Req-Id` header.
//!
//! Three flavors:
//! - `stream_events_consumer`: filters on one `{task_id}` and delivers everything
//!   from sequence 0. Used by `message/stream` once the bootstrap reply has named
//!   the task.
//! - `gateway_stream_events_consumer`: the gateway never sees the bootstrap reply,
//!   because the agent answers the caller's inbox directly, so it cannot narrow by
//!   `task_id` and filters every task instead, demuxing on `Trogon-Req-Id`. It
//!   starts from the stream head observed when the request was admitted, because a
//!   filter that wide cannot afford to replay.
//! - `resubscribe_consumer`: filters on one `{task_id}` and uses `ByStartSequence`
//!   from a client-supplied `last_seq + 1`. Used by `tasks/resubscribe` for
//!   reconnect-after-disconnect, skipping already-seen events without replaying.

use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

use crate::a2a_prefix::A2aPrefix;
use crate::constants::INACTIVE_THRESHOLD;
use crate::nats::subjects::subscriptions::TaskAllEventsSubject;
use crate::nats::subjects::tasks::TaskEventsSubject;
use crate::task_id::A2aTaskId;

/// Durable gateway egress consumer on the full task-events filter.
pub fn gateway_events_consumer(prefix: &A2aPrefix, durable_name: &str, max_ack_pending: i64) -> Config {
    Config {
        durable_name: Some(durable_name.to_string()),
        filter_subject: TaskAllEventsSubject::new(prefix).to_string(),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        max_ack_pending,
        inactive_threshold: INACTIVE_THRESHOLD,
        ..Default::default()
    }
}

pub fn stream_events_consumer(prefix: &A2aPrefix, task_id: &A2aTaskId) -> Config {
    Config {
        filter_subject: TaskEventsSubject::new(prefix, task_id).to_string(),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        max_ack_pending: 256,
        inactive_threshold: INACTIVE_THRESHOLD,
        ..Default::default()
    }
}

/// Gateway-side `message/stream` consumer.
///
/// Unlike [`stream_events_consumer`] this one has no `task_id` to filter on, so it
/// sees every task's events and the pump drops what is not its request. `last_seq`
/// is the events stream head read before the request was forwarded: delivery
/// resumes at the next sequence, so the pump never walks and acks history that
/// predates its own request, and nothing published after admission can slip past
/// the consumer while it is being created.
pub fn gateway_stream_events_consumer(prefix: &A2aPrefix, last_seq: u64, max_ack_pending: i64) -> Config {
    Config {
        filter_subject: TaskAllEventsSubject::new(prefix).to_string(),
        deliver_policy: DeliverPolicy::ByStartSequence {
            start_sequence: last_seq.saturating_add(1),
        },
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        max_ack_pending,
        inactive_threshold: INACTIVE_THRESHOLD,
        ..Default::default()
    }
}

pub fn resubscribe_consumer(prefix: &A2aPrefix, task_id: &A2aTaskId, last_seq: u64) -> Config {
    resubscribe_consumer_with_flow(prefix, task_id, last_seq, 256)
}

pub fn resubscribe_consumer_with_flow(
    prefix: &A2aPrefix,
    task_id: &A2aTaskId,
    last_seq: u64,
    max_ack_pending: i64,
) -> Config {
    Config {
        filter_subject: TaskEventsSubject::new(prefix, task_id).to_string(),
        deliver_policy: DeliverPolicy::ByStartSequence {
            start_sequence: last_seq.saturating_add(1),
        },
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        max_ack_pending,
        inactive_threshold: INACTIVE_THRESHOLD,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
