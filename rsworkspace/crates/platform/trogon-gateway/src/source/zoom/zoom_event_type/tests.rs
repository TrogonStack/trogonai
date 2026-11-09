use super::*;

#[test]
fn empty_error_display_is_specific() {
    let err = ZoomEventType::new("").unwrap_err();
    assert_eq!(err.to_string(), "event_type must not be empty");
}

#[test]
fn invalid_character_error_display_is_specific() {
    let err = ZoomEventType::new("recording completed").unwrap_err();
    assert_eq!(err.to_string(), "event_type contains invalid character: ' '");
}

#[test]
fn too_long_error_display_is_specific() {
    let long_event_type = "a".repeat(129);
    let err = ZoomEventType::new(&long_event_type).unwrap_err();
    assert_eq!(err.to_string(), "event_type is too long: 129 bytes (max 128)");
}

#[test]
fn accepts_dotted_event_types() {
    let event_type = ZoomEventType::new("recording.transcript_completed").unwrap();
    assert_eq!(event_type.as_str(), "recording.transcript_completed");
}

#[test]
fn rejects_empty() {
    assert!(matches!(ZoomEventType::new(""), Err(ZoomEventTypeError::Empty)));
}

#[test]
fn rejects_wildcards() {
    assert!(ZoomEventType::new("recording.*").is_err());
    assert!(ZoomEventType::new("recording.>").is_err());
}

#[test]
fn rejects_whitespace() {
    assert!(ZoomEventType::new("recording completed").is_err());
}

#[test]
fn rejects_malformed_dots() {
    assert!(ZoomEventType::new(".recording").is_err());
    assert!(ZoomEventType::new("recording.").is_err());
    assert!(ZoomEventType::new("recording..completed").is_err());
}

#[test]
fn display_roundtrips() {
    let event_type = ZoomEventType::new("meeting.ended").unwrap();
    assert_eq!(event_type.to_string(), "meeting.ended");
}

#[test]
fn deref_roundtrips_to_str() {
    let event_type = ZoomEventType::new("meeting.ended").unwrap();
    let event_type_str: &str = &event_type;
    assert_eq!(event_type_str, "meeting.ended");
}
