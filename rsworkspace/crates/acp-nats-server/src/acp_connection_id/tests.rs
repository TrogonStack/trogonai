use super::*;
use std::error::Error as _;

#[test]
fn new_wraps_existing_uuid() {
    let uuid = uuid::Uuid::nil();
    assert_eq!(AcpConnectionId::new(uuid).to_string(), uuid.to_string());
}

#[test]
fn parse_round_trips_uuid() {
    let id = AcpConnectionId::default();
    let parsed = AcpConnectionId::parse(&id.to_string()).unwrap();
    assert_eq!(parsed, id);
}

#[test]
fn parse_rejects_invalid_uuid() {
    assert!(AcpConnectionId::parse("not-a-uuid").is_err());
}

#[test]
fn default_generates_non_empty_id() {
    assert!(!AcpConnectionId::default().to_string().is_empty());
}

#[test]
fn parse_error_displays_context() {
    let error = AcpConnectionId::parse("not-a-uuid").unwrap_err();
    assert!(error.to_string().contains("invalid ACP connection id"));
}

#[test]
fn parse_error_exposes_uuid_source() {
    let error = AcpConnectionId::parse("not-a-uuid").unwrap_err();
    assert!(error.source().is_some());
}
