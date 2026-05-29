use std::{borrow::Borrow, str::FromStr};

/// Non-empty metadata header name.
///
/// Header names are stored exactly as provided. Adapters may impose stricter
/// backend-specific limits at their boundary, but empty names and control
/// characters are never valid in the shared event envelope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeaderName(String);

impl HeaderName {
    /// Creates a header name after rejecting invalid input.
    pub fn new(name: impl Into<String>) -> Result<Self, HeaderNameError> {
        let name = name.into();
        if name.is_empty() {
            return Err(HeaderNameError::Empty);
        }
        if name.chars().any(char::is_control) {
            return Err(HeaderNameError::ContainsControlCharacter);
        }
        Ok(Self(name))
    }

    /// Returns the header name as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for HeaderName {
    type Err = HeaderNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for HeaderName {
    type Error = HeaderNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for HeaderName {
    type Error = HeaderNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for HeaderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for HeaderName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when constructing an invalid [`HeaderName`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderNameError {
    /// Header names cannot be empty.
    Empty,
    /// Header names cannot contain control characters.
    ContainsControlCharacter,
}

impl std::fmt::Display for HeaderNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("event header name cannot be empty"),
            Self::ContainsControlCharacter => f.write_str("event header name cannot contain control characters"),
        }
    }
}

impl std::error::Error for HeaderNameError {}
