use std::fmt;

use trogon_nats::DottedNatsToken;
use trogon_nats::SubjectTokenViolationError;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NotionEventTypeError {
    #[error("type must not be empty")]
    Empty,
    #[error("type contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("type is too long: {0} bytes (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolationError> for NotionEventTypeError {
    fn from(violation: SubjectTokenViolationError) -> Self {
        match violation {
            SubjectTokenViolationError::Empty => Self::Empty,
            SubjectTokenViolationError::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolationError::TooLong(len) => Self::TooLong(len),
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
mod tests;
