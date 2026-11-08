use super::*;

#[test]
fn exposes_its_reason_and_names_it() {
    let error = AuditUnavailableError::new(FailureReason::StreamFull);
    assert_eq!(error.reason(), FailureReason::StreamFull);
    assert_eq!(error.to_string(), "audit is unavailable: stream_full");
}
