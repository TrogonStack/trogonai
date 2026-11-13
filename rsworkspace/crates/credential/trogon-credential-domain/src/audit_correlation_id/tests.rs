use super::*;

#[test]
fn refuses_an_empty_id() {
    assert_eq!(AuditCorrelationId::new(""), Err(AuditCorrelationIdError::Empty));
}

#[test]
fn refuses_a_space_because_the_id_rides_a_header() {
    assert_eq!(
        AuditCorrelationId::new("use 1"),
        Err(AuditCorrelationIdError::InvalidCharacter(' '))
    );
}

#[test]
fn refuses_a_line_break_that_would_split_a_header() {
    assert_eq!(
        AuditCorrelationId::new("use-1\r\nX-Vault-Token: forged"),
        Err(AuditCorrelationIdError::InvalidCharacter('\r'))
    );
}

#[test]
fn refuses_non_ascii() {
    assert_eq!(
        AuditCorrelationId::new("use-ü"),
        Err(AuditCorrelationIdError::InvalidCharacter('ü'))
    );
}

#[test]
fn refuses_an_id_past_the_bound() {
    let id = "a".repeat(MAX_AUDIT_CORRELATION_ID_LEN + 1);

    assert_eq!(
        AuditCorrelationId::new(id),
        Err(AuditCorrelationIdError::TooLong(MAX_AUDIT_CORRELATION_ID_LEN + 1))
    );
}

#[test]
fn accepts_an_id_at_the_bound() {
    let id = "a".repeat(MAX_AUDIT_CORRELATION_ID_LEN);

    assert_eq!(AuditCorrelationId::new(id.as_str()).unwrap().as_str(), id);
}

#[test]
fn keeps_the_id_whole() {
    let id = AuditCorrelationId::new("use-7f3a:trace/9_~").unwrap();

    assert_eq!(id.as_str(), "use-7f3a:trace/9_~");
    assert_eq!(id.to_string(), "use-7f3a:trace/9_~");
}

#[test]
fn error_says_what_was_wrong() {
    assert_eq!(
        AuditCorrelationIdError::Empty.to_string(),
        "audit correlation id must not be empty"
    );
    assert_eq!(
        AuditCorrelationIdError::InvalidCharacter(' ').to_string(),
        "audit correlation id contains invalid character ' '"
    );
    assert_eq!(
        AuditCorrelationIdError::TooLong(129).to_string(),
        "audit correlation id exceeds maximum length: 129"
    );
}
