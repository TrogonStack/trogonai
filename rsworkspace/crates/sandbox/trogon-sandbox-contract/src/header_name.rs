use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::HeaderNameError;

/// A header field name, compared case-insensitively and kept lowercase.
///
/// It is a non-empty token of at most [`HeaderName::MAX_LEN`] bytes. It serializes as a JSON string, and
/// construction, parsing, and deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HeaderName(String);

impl HeaderName {
    /// Longest header name the wire accepts, in bytes.
    pub const MAX_LEN: usize = 256;

    pub fn new(value: impl Into<String>) -> Result<Self, HeaderNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(HeaderNameError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(HeaderNameError::TooLong { max: Self::MAX_LEN });
        }
        let is_token = |byte: u8| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
        if !value.bytes().all(is_token) {
            return Err(HeaderNameError::NotAToken);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HeaderName {
    type Error = HeaderNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for HeaderName {
    type Err = HeaderNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<HeaderName> for String {
    fn from(name: HeaderName) -> Self {
        name.0
    }
}

impl fmt::Display for HeaderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
