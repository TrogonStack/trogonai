use super::*;

#[test]
fn empty_error_display_is_specific() {
    let err = IncidentioEventType::new("").unwrap_err();
    assert_eq!(err.to_string(), "event_type must not be empty");
}

#[test]
fn invalid_character_error_display_is_specific() {
    let err = IncidentioEventType::new("public incident").unwrap_err();
    assert_eq!(err.to_string(), "event_type contains invalid character: ' '");
}

#[test]
fn too_long_error_display_is_specific() {
    let long_event_type = "a".repeat(129);
    let err = IncidentioEventType::new(&long_event_type).unwrap_err();
    assert_eq!(err.to_string(), "event_type is too long: 129 bytes (max 128)");
}

#[test]
fn accepts_dotted_event_types() {
    let event_type = IncidentioEventType::new("public_incident.incident_created_v2").unwrap();
    assert_eq!(event_type.as_str(), "public_incident.incident_created_v2");
}

#[test]
fn rejects_empty() {
    assert!(matches!(
        IncidentioEventType::new(""),
        Err(IncidentioEventTypeError::Empty)
    ));
}

#[test]
fn rejects_wildcards() {
    assert!(IncidentioEventType::new("public_incident.*").is_err());
    assert!(IncidentioEventType::new("public_incident.>").is_err());
}

#[test]
fn rejects_whitespace() {
    assert!(IncidentioEventType::new("public incident").is_err());
}

#[test]
fn rejects_malformed_dots() {
    assert!(IncidentioEventType::new(".incident").is_err());
    assert!(IncidentioEventType::new("incident.").is_err());
    assert!(IncidentioEventType::new("incident..created").is_err());
}

#[test]
fn display_roundtrips() {
    let event_type = IncidentioEventType::new("private_incident.incident_updated_v2").unwrap();
    assert_eq!(event_type.to_string(), "private_incident.incident_updated_v2");
}

#[test]
fn deref_roundtrips_to_str() {
    let event_type = IncidentioEventType::new("private_incident.incident_updated_v2").unwrap();
    let event_type_str: &str = &event_type;
    assert_eq!(event_type_str, "private_incident.incident_updated_v2");
}
