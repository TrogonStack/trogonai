use std::fmt;

use trogon_nats::DottedNatsToken;
use trogon_nats::SubjectTokenViolation;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IncidentioEventTypeError {
    #[error("event_type must not be empty")]
    Empty,
    #[error("event_type contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("event_type is too long: {0} bytes (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for IncidentioEventTypeError {
    fn from(violation: SubjectTokenViolation) -> Self {
        match violation {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IncidentioEventType(DottedNatsToken);

impl IncidentioEventType {
    pub fn new(value: impl AsRef<str>) -> Result<Self, IncidentioEventTypeError> {
        DottedNatsToken::new(value)
            .map(Self)
            .map_err(IncidentioEventTypeError::from)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for IncidentioEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for IncidentioEventType {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
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
}
