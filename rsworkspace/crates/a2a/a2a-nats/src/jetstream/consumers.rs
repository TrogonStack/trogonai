//! JetStream consumer configs for A2A task event delivery.
//!
//! Two flavors:
//! - `stream_events_consumer`: filters on `{req_id}` across any task_id and
//!   delivers everything from sequence 0. Used by `message/stream` on initial
//!   subscription, when the caller has no prior cursor.
//! - `resubscribe_consumer`: filters on `{task_id}` (any `req_id`) and uses
//!   `ByStartSequence` from a client-supplied `last_seq + 1`. Used by `tasks/resubscribe`
//!   for reconnect-after-disconnect — skips already-seen events without re-replaying them.

use std::time::Duration;

use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

use crate::a2a_prefix::A2aPrefix;
use crate::req_id::ReqId;
use crate::task_id::A2aTaskId;

const INACTIVE_THRESHOLD: Duration = Duration::from_secs(300);

/// Durable gateway egress consumer on the full task-events filter.
pub fn gateway_events_consumer(prefix: &A2aPrefix, durable_name: &str, max_ack_pending: i64) -> Config {
    let pfx = prefix.as_str();
    Config {
        durable_name: Some(durable_name.to_string()),
        filter_subject: format!("{pfx}.tasks.*.events.*"),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        max_ack_pending,
        inactive_threshold: INACTIVE_THRESHOLD,
        ..Default::default()
    }
}

pub fn stream_events_consumer(prefix: &A2aPrefix, req_id: &ReqId) -> Config {
    gateway_stream_events_consumer(prefix, req_id, 256)
}

pub fn gateway_stream_events_consumer(prefix: &A2aPrefix, req_id: &ReqId, max_ack_pending: i64) -> Config {
    let pfx = prefix.as_str();
    Config {
        filter_subject: format!("{pfx}.tasks.*.events.{req_id}"),
        deliver_policy: DeliverPolicy::All,
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
    let pfx = prefix.as_str();
    Config {
        filter_subject: format!("{pfx}.tasks.{task_id}.events.*"),
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
