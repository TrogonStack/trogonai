use super::*;

#[test]
fn says_why_the_record_was_not_acknowledged() {
    let error = ReleaseAuditError::new("stream full");

    assert_eq!(error.message(), "stream full");
    assert_eq!(error.to_string(), "audit record was not acknowledged: stream full");
}
