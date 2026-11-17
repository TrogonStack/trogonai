use async_nats::jetstream::context::{PublishError, PublishErrorKind};
use trogon_nats::mocks::MockError;

use super::*;
use crate::fixtures::server_error;

const STREAM_STORE_FAILED: u64 = 10077;

#[test]
fn a_timeout_is_a_timeout() {
    assert_eq!(
        PublishError::new(PublishErrorKind::TimedOut).classify(),
        SinkError::Timeout
    );
}

#[test]
fn a_stream_store_failure_is_a_full_stream() {
    let error = PublishError::with_source(PublishErrorKind::Other, server_error(503, STREAM_STORE_FAILED));
    assert_eq!(error.classify(), SinkError::StreamFull);
}

#[test]
fn another_server_error_is_unreachable() {
    let error = PublishError::with_source(PublishErrorKind::Other, server_error(404, 10059));
    assert_eq!(error.classify(), SinkError::Unreachable);
}

#[test]
fn an_error_without_a_server_cause_is_unreachable() {
    assert_eq!(
        PublishError::new(PublishErrorKind::Other).classify(),
        SinkError::Unreachable
    );
    let foreign = PublishError::with_source(PublishErrorKind::Other, MockError("io".to_owned()));
    assert_eq!(foreign.classify(), SinkError::Unreachable);
}

#[test]
fn other_kinds_are_unreachable() {
    for kind in [
        PublishErrorKind::StreamNotFound,
        PublishErrorKind::BrokenPipe,
        PublishErrorKind::MaxAckPending,
    ] {
        assert_eq!(PublishError::new(kind).classify(), SinkError::Unreachable);
    }
}

#[test]
fn the_test_double_error_is_unreachable() {
    assert_eq!(MockError("down".to_owned()).classify(), SinkError::Unreachable);
}
