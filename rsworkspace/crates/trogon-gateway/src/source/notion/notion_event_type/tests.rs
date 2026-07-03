use super::*;

#[test]
fn empty_error_display_is_specific() {
    let err = NotionEventType::new("").unwrap_err();
    assert_eq!(err.to_string(), "type must not be empty");
}

#[test]
fn invalid_character_error_display_is_specific() {
    let err = NotionEventType::new("page created").unwrap_err();
    assert_eq!(err.to_string(), "type contains invalid character: ' '");
}

#[test]
fn too_long_error_display_is_specific() {
    let long_event_type = "a".repeat(129);
    let err = NotionEventType::new(&long_event_type).unwrap_err();
    assert_eq!(err.to_string(), "type is too long: 129 bytes (max 128)");
}

#[test]
fn accepts_dotted_event_types() {
    let event_type = NotionEventType::new("page.created").unwrap();
    assert_eq!(event_type.as_str(), "page.created");
}

#[test]
fn deref_coerces_to_str() {
    let event_type = NotionEventType::new("page.created").unwrap();
    assert_eq!(&*event_type, "page.created");
}

#[test]
fn rejects_wildcards() {
    assert!(NotionEventType::new("page.*").is_err());
    assert!(NotionEventType::new("page.>").is_err());
}

#[test]
fn rejects_malformed_dots() {
    assert!(NotionEventType::new(".page").is_err());
    assert!(NotionEventType::new("page.").is_err());
    assert!(NotionEventType::new("page..created").is_err());
}
