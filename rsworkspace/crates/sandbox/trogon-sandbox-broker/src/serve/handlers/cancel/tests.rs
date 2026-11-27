use super::*;

#[test]
fn an_empty_body_is_a_plain_cancel() {
    assert_eq!(cancel_reason(b""), LeaseReleasedReason::Cancelled);
}

#[test]
fn a_worker_may_report_a_failed_sandbox() {
    assert_eq!(cancel_reason(br#""failed""#), LeaseReleasedReason::Failed);
}

#[test]
fn no_other_reason_can_be_claimed() {
    assert_eq!(cancel_reason(br#""expired""#), LeaseReleasedReason::Cancelled);
    assert_eq!(cancel_reason(br#""completed""#), LeaseReleasedReason::Cancelled);
    assert_eq!(cancel_reason(b"garbage"), LeaseReleasedReason::Cancelled);
}
