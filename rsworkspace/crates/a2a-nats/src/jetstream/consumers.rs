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
pub fn gateway_events_consumer(
    prefix: &A2aPrefix,
    durable_name: &str,
    max_ack_pending: i64,
) -> Config {
    let pfx = prefix.as_str();
    Config {
        durable_name: Some(durable_name.to_string()),
        filter_subject: format!("{pfx}.task.*.events.*"),
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
        filter_subject: format!("{pfx}.task.*.events.{req_id}"),
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
        filter_subject: format!("{pfx}.task.{task_id}.events.*"),
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
mod tests {
    use super::*;

    fn p(s: &str) -> A2aPrefix {
        A2aPrefix::new(s.to_string()).expect("test prefix")
    }

    fn tid(s: &str) -> A2aTaskId {
        A2aTaskId::new(s).expect("test task id")
    }

    fn rid(s: &str) -> ReqId {
        ReqId::from_test(s)
    }

    #[test]
    fn gateway_events_consumer_uses_durable_full_task_filter() {
        let config = gateway_events_consumer(&p("a2a"), "A2A_GATEWAY_EVENTS", 1024);
        assert_eq!(config.durable_name.as_deref(), Some("A2A_GATEWAY_EVENTS"));
        assert_eq!(config.filter_subject, "a2a.task.*.events.*");
        assert_eq!(config.max_ack_pending, 1024);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
    }

    #[test]
    fn stream_events_consumer_filter_uses_wildcard_task_id() {
        let config = stream_events_consumer(&p("a2a"), &rid("r1"));
        assert_eq!(config.filter_subject, "a2a.task.*.events.r1");
    }

    #[test]
    fn stream_events_consumer_delivers_all() {
        let config = stream_events_consumer(&p("a2a"), &rid("r1"));
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn resubscribe_consumer_filter_matches_all_req_ids() {
        let config = resubscribe_consumer(&p("a2a"), &tid("t1"), 100);
        assert_eq!(config.filter_subject, "a2a.task.t1.events.*");
    }

    #[test]
    fn resubscribe_consumer_starts_at_last_seq_plus_one() {
        let config = resubscribe_consumer(&p("a2a"), &tid("t1"), 42);
        assert_eq!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence { start_sequence: 43 }
        );
    }

    #[test]
    fn resubscribe_consumer_handles_zero_seq() {
        let config = resubscribe_consumer(&p("a2a"), &tid("t1"), 0);
        assert_eq!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence { start_sequence: 1 }
        );
    }

    #[test]
    fn resubscribe_consumer_saturates_at_u64_max() {
        let config = resubscribe_consumer(&p("a2a"), &tid("t1"), u64::MAX);
        assert_eq!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence {
                start_sequence: u64::MAX
            }
        );
    }

    #[test]
    fn consumers_use_explicit_ack_and_instant_replay() {
        let stream = stream_events_consumer(&p("a2a"), &rid("r1"));
        let resub = resubscribe_consumer(&p("a2a"), &tid("t1"), 1);
        assert_eq!(stream.ack_policy, AckPolicy::Explicit);
        assert_eq!(stream.replay_policy, ReplayPolicy::Instant);
        assert_eq!(resub.ack_policy, AckPolicy::Explicit);
        assert_eq!(resub.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn custom_prefix_in_consumers() {
        let stream = stream_events_consumer(&p("myapp"), &rid("r1"));
        assert_eq!(stream.filter_subject, "myapp.task.*.events.r1");
        let resub = resubscribe_consumer(&p("myapp"), &tid("t1"), 5);
        assert_eq!(resub.filter_subject, "myapp.task.t1.events.*");
    }

    #[test]
    fn consumers_set_five_minute_inactive_threshold() {
        let stream = stream_events_consumer(&p("a2a"), &rid("r1"));
        let resub = resubscribe_consumer(&p("a2a"), &tid("t1"), 0);
        assert_eq!(stream.inactive_threshold, Duration::from_secs(300));
        assert_eq!(resub.inactive_threshold, Duration::from_secs(300));
    }
}
