use std::time::Duration;

use async_nats::jetstream::consumer::StreamErrorKind;
use async_nats::jetstream::consumer::pull::OrderedErrorKind;
use async_nats::jetstream::stream::ConsumerErrorKind;

use super::{ReplayRetryPolicy, RetryDecision, is_empty_replay_range, is_transient_replay_error};
use crate::stream_store::{ReadStreamError, StreamStoreError};

#[test]
fn is_empty_replay_range_rejects_invalid_bounds() {
    assert!(is_empty_replay_range(0, 1));
    assert!(is_empty_replay_range(1, 0));
    assert!(is_empty_replay_range(2, 1));
    assert!(!is_empty_replay_range(1, 1));
    assert!(!is_empty_replay_range(1, 3));
}

#[test]
fn retry_policy_gives_up_after_max_attempts() {
    let policy = ReplayRetryPolicy::new(3, Duration::from_millis(1), Duration::from_secs(1));

    assert!(matches!(policy.decide(0), RetryDecision::Retry { .. }));
    assert!(matches!(policy.decide(1), RetryDecision::Retry { .. }));
    assert!(matches!(policy.decide(2), RetryDecision::Retry { .. }));
    assert_eq!(policy.decide(3), RetryDecision::GiveUp);
    assert_eq!(policy.decide(100), RetryDecision::GiveUp);
}

#[test]
fn retry_policy_backoff_grows_and_caps() {
    let policy = ReplayRetryPolicy::new(10, Duration::from_millis(10), Duration::from_millis(100));

    assert_eq!(
        policy.decide(0),
        RetryDecision::Retry {
            delay: Duration::from_millis(10)
        }
    );
    assert_eq!(
        policy.decide(1),
        RetryDecision::Retry {
            delay: Duration::from_millis(20)
        }
    );
    assert_eq!(
        policy.decide(2),
        RetryDecision::Retry {
            delay: Duration::from_millis(40)
        }
    );
    assert_eq!(
        policy.decide(3),
        RetryDecision::Retry {
            delay: Duration::from_millis(80)
        }
    );
    assert_eq!(
        policy.decide(4),
        RetryDecision::Retry {
            delay: Duration::from_millis(100)
        }
    );
    assert_eq!(
        policy.decide(9),
        RetryDecision::Retry {
            delay: Duration::from_millis(100)
        }
    );
}

#[test]
fn retry_policy_default_is_bounded_and_never_zero_wait() {
    let policy = ReplayRetryPolicy::default();

    let RetryDecision::Retry { delay } = policy.decide(0) else {
        panic!("default policy should allow a first retry");
    };
    assert!(delay > Duration::ZERO);
}

#[test]
fn is_transient_replay_error_classifies_create_consumer_kinds() {
    assert!(is_transient_replay_error(&create_consumer_error(
        ConsumerErrorKind::TimedOut
    )));
    assert!(is_transient_replay_error(&create_consumer_error(
        ConsumerErrorKind::Request
    )));
    assert!(!is_transient_replay_error(&create_consumer_error(
        ConsumerErrorKind::InvalidConsumerType
    )));
    assert!(!is_transient_replay_error(&create_consumer_error(
        ConsumerErrorKind::InvalidName
    )));
    assert!(!is_transient_replay_error(&create_consumer_error(
        ConsumerErrorKind::Other
    )));
}

#[test]
fn is_transient_replay_error_classifies_open_message_stream_kinds() {
    assert!(is_transient_replay_error(&open_message_stream_error(
        StreamErrorKind::TimedOut
    )));
    assert!(!is_transient_replay_error(&open_message_stream_error(
        StreamErrorKind::Other
    )));
}

#[test]
fn is_transient_replay_error_classifies_read_message_kinds() {
    assert!(is_transient_replay_error(&read_message_error(OrderedErrorKind::Pull)));
    assert!(!is_transient_replay_error(&read_message_error(
        OrderedErrorKind::PushBasedConsumer
    )));
    assert!(!is_transient_replay_error(&read_message_error(OrderedErrorKind::Other)));
}

#[test]
fn is_transient_replay_error_treats_replay_ended_before_target_as_transient() {
    let error: StreamStoreError = ReadStreamError::ReplayEndedBeforeTarget { to_sequence: 9 }.into();

    assert!(is_transient_replay_error(&error));
}

#[test]
fn is_transient_replay_error_treats_non_read_errors_as_fatal() {
    assert!(!is_transient_replay_error(&StreamStoreError::WrongExpectedVersion));
}

fn create_consumer_error(kind: ConsumerErrorKind) -> StreamStoreError {
    ReadStreamError::CreateReplayConsumer { source: kind.into() }.into()
}

fn open_message_stream_error(kind: StreamErrorKind) -> StreamStoreError {
    ReadStreamError::OpenReplayMessageStream { source: kind.into() }.into()
}

fn read_message_error(kind: OrderedErrorKind) -> StreamStoreError {
    ReadStreamError::ReadReplayMessage { source: kind.into() }.into()
}
