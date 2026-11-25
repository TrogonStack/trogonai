use super::*;

#[test]
fn names_every_reason() {
    let expected = [
        (FailureReason::Closed, "closed"),
        (FailureReason::Unreachable, "unreachable"),
        (FailureReason::Timeout, "timeout"),
        (FailureReason::StreamFull, "stream_full"),
        (FailureReason::Degraded, "degraded"),
        (FailureReason::QueueFull, "queue_full"),
        (FailureReason::BufferFull, "buffer_full"),
        (FailureReason::TooLarge, "too_large"),
    ];
    for (reason, name) in expected {
        assert_eq!(reason.as_str(), name);
        assert_eq!(reason.to_string(), name);
    }
}
