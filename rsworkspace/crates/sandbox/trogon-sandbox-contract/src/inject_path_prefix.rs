use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::InjectPathPrefixError;

/// An absolute path prefix a credential may be injected under, matched on whole segments.
///
/// `/v1` covers `/v1` and `/v1/users`, never `/v10`. The prefix is compared byte for byte, so it carries no percent
/// escapes. It is `/`, or absolute segments with no empty, `.`, or `..` segment and no trailing slash. It holds at
/// most [`InjectPathPrefix::MAX_LEN`] bytes of ASCII letters, digits, unreserved characters, sub-delimiters, `:`,
/// `@`, and `/`. It serializes as a JSON string, and construction, parsing, and deserialization all enforce the
/// grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InjectPathPrefix(String);

impl InjectPathPrefix {
    /// Longest path prefix the wire accepts, in bytes.
    pub const MAX_LEN: usize = 1024;

    pub fn new(value: impl Into<String>) -> Result<Self, InjectPathPrefixError> {
        let value = value.into();
        if !value.starts_with('/') {
            return Err(InjectPathPrefixError::NotAbsolute);
        }
        if value.len() > Self::MAX_LEN {
            return Err(InjectPathPrefixError::TooLong { max: Self::MAX_LEN });
        }
        if !value.bytes().all(is_prefix_byte) {
            return Err(InjectPathPrefixError::Character);
        }
        if value == "/" {
            return Ok(Self(value));
        }
        if value.ends_with('/') {
            return Err(InjectPathPrefixError::TrailingSlash);
        }
        if value[1..].split('/').any(|segment| matches!(segment, "" | "." | "..")) {
            return Err(InjectPathPrefixError::Segment);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_prefix_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&byte)
}

impl TryFrom<String> for InjectPathPrefix {
    type Error = InjectPathPrefixError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for InjectPathPrefix {
    type Err = InjectPathPrefixError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<InjectPathPrefix> for String {
    fn from(prefix: InjectPathPrefix) -> Self {
        prefix.0
    }
}

impl fmt::Display for InjectPathPrefix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
