use super::*;

#[test]
fn maps_to_the_reason_it_names() {
    assert_eq!(SinkError::Unreachable.reason(), FailureReason::Unreachable);
    assert_eq!(SinkError::Timeout.reason(), FailureReason::Timeout);
    assert_eq!(SinkError::StreamFull.reason(), FailureReason::StreamFull);
}

#[test]
fn a_full_stream_reads_differently_from_an_unreachable_one() {
    assert_ne!(SinkError::StreamFull.to_string(), SinkError::Unreachable.to_string());
    assert_ne!(SinkError::Timeout.to_string(), SinkError::Unreachable.to_string());
}
