use super::*;

#[test]
fn empty_error_display_is_specific() {
    let err = PlaidEventType::new("").unwrap_err();
    assert_eq!(err.to_string(), "event_type must not be empty");
}

#[test]
fn invalid_character_error_display_is_specific() {
    let err = PlaidEventType::new("TRANSACTIONS SYNC").unwrap_err();
    assert_eq!(err.to_string(), "event_type contains invalid character: ' '");
}

#[test]
fn too_long_error_display_is_specific() {
    let err = PlaidEventType::new("a".repeat(129)).unwrap_err();
    assert_eq!(err.to_string(), "event_type is too long: 129 bytes (max 128)");
}

#[test]
fn from_webhook_joins_type_and_code_verbatim() {
    let event_type = PlaidEventType::from_webhook("TRANSACTIONS", "SYNC_UPDATES_AVAILABLE").unwrap();
    assert_eq!(event_type.as_str(), "TRANSACTIONS.SYNC_UPDATES_AVAILABLE");
}

#[test]
fn from_webhook_rejects_empty_parts() {
    assert!(PlaidEventType::from_webhook("", "SYNC_UPDATES_AVAILABLE").is_err());
    assert!(PlaidEventType::from_webhook("TRANSACTIONS", "").is_err());
}

#[test]
fn from_webhook_rejects_wildcards_and_whitespace() {
    assert!(PlaidEventType::from_webhook("TRANSACTIONS", "*").is_err());
    assert!(PlaidEventType::from_webhook("ITEM", ">").is_err());
    assert!(PlaidEventType::from_webhook("ITEM", "PENDING EXPIRATION").is_err());
}

#[test]
fn rejects_empty() {
    assert!(matches!(PlaidEventType::new(""), Err(PlaidEventTypeError::Empty)));
}

#[test]
fn rejects_malformed_dots() {
    assert!(PlaidEventType::new(".ITEM").is_err());
    assert!(PlaidEventType::new("ITEM.").is_err());
    assert!(PlaidEventType::new("ITEM..ERROR").is_err());
}

#[test]
fn display_and_deref_roundtrip() {
    let event_type = PlaidEventType::new("ITEM.ERROR").unwrap();
    assert_eq!(event_type.to_string(), "ITEM.ERROR");
    let as_str: &str = &event_type;
    assert_eq!(as_str, "ITEM.ERROR");
}
