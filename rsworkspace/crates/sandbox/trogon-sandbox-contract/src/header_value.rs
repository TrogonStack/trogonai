use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::HeaderValueError;

/// A header field value to compare against.
///
/// Surrounding spaces and tabs are trimmed. What remains holds 1 to [`HeaderValue::MAX_LEN`] bytes and no control
/// characters other than a tab. It serializes as a JSON string, and construction, parsing, and deserialization all
/// enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HeaderValue(String);

impl HeaderValue {
    /// Longest header value the wire accepts, in bytes.
    pub const MAX_LEN: usize = 8192;

    pub fn new(value: impl AsRef<str>) -> Result<Self, HeaderValueError> {
        let trimmed = value.as_ref().trim_matches([' ', '\t']);
        if trimmed.is_empty() {
            return Err(HeaderValueError::Empty);
        }
        if trimmed.len() > Self::MAX_LEN {
            return Err(HeaderValueError::TooLong { max: Self::MAX_LEN });
        }
        if trimmed
            .bytes()
            .any(|byte| (byte < 0x20 && byte != b'\t') || byte == 0x7f)
        {
            return Err(HeaderValueError::ControlCharacter);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HeaderValue {
    type Error = HeaderValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for HeaderValue {
    type Err = HeaderValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<HeaderValue> for String {
    fn from(value: HeaderValue) -> Self {
        value.0
    }
}

impl fmt::Display for HeaderValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
