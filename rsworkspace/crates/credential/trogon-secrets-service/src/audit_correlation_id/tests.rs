use super::*;

#[test]
fn refuses_an_empty_id() {
    assert_eq!(AuditCorrelationId::new(""), Err(AuditCorrelationIdError::Empty));
}

#[test]
fn keeps_the_id_whole() {
    let id = AuditCorrelationId::new("use-7f3a").unwrap();

    assert_eq!(id.as_str(), "use-7f3a");
    assert_eq!(id.to_string(), "use-7f3a");
}

#[test]
fn error_says_what_was_wrong() {
    assert_eq!(
        AuditCorrelationIdError::Empty.to_string(),
        "audit correlation id must not be empty"
    );
}
