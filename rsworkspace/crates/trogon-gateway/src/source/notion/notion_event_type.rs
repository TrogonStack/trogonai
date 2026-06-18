use std::fmt;

use trogon_nats::DottedNatsToken;
use trogon_nats::SubjectTokenViolation;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NotionEventTypeError {
    #[error("type must not be empty")]
    Empty,
    #[error("type contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("type is too long: {0} bytes (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for NotionEventTypeError {
    fn from(violation: SubjectTokenViolation) -> Self {
        match violation {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NotionEventType(DottedNatsToken);

impl NotionEventType {
    pub fn new(value: impl AsRef<str>) -> Result<Self, NotionEventTypeError> {
        DottedNatsToken::new(value)
            .map(Self)
            .map_err(NotionEventTypeError::from)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for NotionEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for NotionEventType {
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
}
