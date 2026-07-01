use super::*;

#[test]
fn empty_error_display_is_specific() {
    let err = DatadogEventType::new("").unwrap_err();
    assert_eq!(err.to_string(), "event_type must not be empty");
}

#[test]
fn invalid_character_error_display_is_specific() {
    let err = DatadogEventType::new("metric alert").unwrap_err();
    assert_eq!(err.to_string(), "event_type contains invalid character: ' '");
}

#[test]
fn too_long_error_display_is_specific() {
    let long_event_type = "a".repeat(129);
    let err = DatadogEventType::new(&long_event_type).unwrap_err();
    assert_eq!(err.to_string(), "event_type is too long: 129 bytes (max 128)");
}

#[test]
fn accepts_datadog_event_types() {
    let event_type = DatadogEventType::new("metric_alert_monitor").unwrap();
    assert_eq!(event_type.as_str(), "metric_alert_monitor");
}

#[test]
fn accepts_dotted_event_types() {
    let event_type = DatadogEventType::new("monitor.triggered").unwrap();
    assert_eq!(event_type.as_str(), "monitor.triggered");
}

#[test]
fn rejects_empty() {
    assert!(matches!(DatadogEventType::new(""), Err(DatadogEventTypeError::Empty)));
}

#[test]
fn rejects_wildcards() {
    assert!(DatadogEventType::new("monitor.*").is_err());
    assert!(DatadogEventType::new("monitor.>").is_err());
}

#[test]
fn rejects_whitespace() {
    assert!(DatadogEventType::new("metric alert").is_err());
}

#[test]
fn rejects_malformed_dots() {
    assert!(DatadogEventType::new(".monitor").is_err());
    assert!(DatadogEventType::new("monitor.").is_err());
    assert!(DatadogEventType::new("monitor..triggered").is_err());
}

#[test]
fn display_roundtrips() {
    let event_type = DatadogEventType::new("event_alert").unwrap();
    assert_eq!(event_type.to_string(), "event_alert");
}

#[test]
fn deref_roundtrips_to_str() {
    let event_type = DatadogEventType::new("event_alert").unwrap();
    let event_type_str: &str = &event_type;
    assert_eq!(event_type_str, "event_alert");
}
