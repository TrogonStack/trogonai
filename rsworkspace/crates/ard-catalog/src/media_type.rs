//! ARD catalog entry media type value object.

use std::sync::Arc;

/// Error returned when [`MediaType`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MediaTypeError {
    #[error("media type must not be empty")]
    Empty,
    #[error("media type must not contain whitespace")]
    ContainsWhitespace,
    #[error("media type must contain a type and subtype separated by '/'")]
    MissingSubtypeSeparator,
}

/// Preserves ARD `type` values exactly, including future media types.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MediaType(Arc<str>);

impl MediaType {
    pub fn new(s: impl AsRef<str>) -> Result<Self, MediaTypeError> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(MediaTypeError::Empty);
        }
        if s.chars().any(char::is_whitespace) {
            return Err(MediaTypeError::ContainsWhitespace);
        }
        let (type_part, subtype_part) = s.split_once('/').ok_or(MediaTypeError::MissingSubtypeSeparator)?;
        if type_part.is_empty() || subtype_part.is_empty() {
            return Err(MediaTypeError::MissingSubtypeSeparator);
        }
        Ok(Self(Arc::from(s)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests;
