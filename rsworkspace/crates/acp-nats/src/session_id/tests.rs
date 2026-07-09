use super::*;
use agent_client_protocol::schema::v1::SessionId;

#[test]
fn acp_session_id_too_long_returns_err() {
    let long_id = "a".repeat(129);
    let err = AcpSessionId::new(&long_id).err().unwrap();
    assert_eq!(err, SessionIdError::TooLong(129));

    assert!(AcpSessionId::new("a".repeat(128).as_str()).is_ok());
}

#[test]
fn acp_session_id_rejects_dots() {
    let err = AcpSessionId::new("session.id").err().unwrap();
    assert_eq!(err, SessionIdError::InvalidCharacter('.'));
}

#[test]
fn acp_session_id_rejects_wildcards() {
    assert!(AcpSessionId::new("session*").is_err());
    assert!(AcpSessionId::new("session>").is_err());
    assert!(AcpSessionId::new(">").is_err());
}

#[test]
fn acp_session_id_rejects_non_ascii() {
    let err = AcpSessionId::new("séssion").err().unwrap();
    assert_eq!(err, SessionIdError::InvalidCharacter('é'));
}

#[test]
fn acp_session_id_valid() {
    assert!(AcpSessionId::new("valid-session-123").is_ok());
    assert!(AcpSessionId::new("a").is_ok());
}

#[test]
fn acp_session_id_empty_returns_err() {
    let err = AcpSessionId::new("").err().unwrap();
    assert_eq!(err, SessionIdError::Empty);
}

#[test]
fn acp_session_id_try_from_session_id() {
    let valid = SessionId::from("valid-session");
    assert!(AcpSessionId::try_from(&valid).is_ok());
    assert_eq!(AcpSessionId::try_from(&valid).unwrap().as_str(), "valid-session");

    let invalid = SessionId::from("invalid.session");
    assert!(AcpSessionId::try_from(&invalid).is_err());
}

#[test]
fn acp_session_id_display_and_deref() {
    let id = AcpSessionId::new("my-session").unwrap();
    assert_eq!(format!("{}", id), "my-session");
    assert_eq!(id.len(), 10);
    assert!(id.starts_with("my"));
}

#[test]
fn session_id_error_display() {
    assert_eq!(format!("{}", SessionIdError::Empty), "session_id must not be empty");
    assert_eq!(
        format!("{}", SessionIdError::InvalidCharacter('.')),
        "session_id contains invalid character: '.'"
    );
    assert_eq!(
        format!("{}", SessionIdError::TooLong(129)),
        "session_id is too long: 129 characters (max 128)"
    );
}
