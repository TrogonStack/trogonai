use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::PathPrefixError;

/// A request path prefix.
///
/// It is `/` or absolute segments without a query, a fragment, a backslash, a percent escape, or dot segments, and
/// it holds at most [`PathPrefix::MAX_LEN`] bytes. A trailing slash is dropped, so `/v1/` and `/v1` are the same
/// prefix. It serializes as a JSON string, and construction, parsing, and deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PathPrefix(String);

impl PathPrefix {
    /// Longest path prefix the wire accepts, in bytes.
    pub const MAX_LEN: usize = 2048;

    pub fn new(value: impl Into<String>) -> Result<Self, PathPrefixError> {
        let value = value.into();
        if !value.starts_with('/') {
            return Err(PathPrefixError::NotAbsolute);
        }
        if value.len() > Self::MAX_LEN {
            return Err(PathPrefixError::TooLong { max: Self::MAX_LEN });
        }
        if let Some(bad) = value
            .chars()
            .find(|character| !character.is_ascii_graphic() || matches!(character, '?' | '#' | '\\' | '%'))
        {
            return Err(PathPrefixError::Forbidden(bad));
        }
        let trimmed = value.strip_suffix('/').unwrap_or(&value);
        if trimmed
            .split('/')
            .skip(1)
            .any(|segment| matches!(segment, "" | "." | ".."))
        {
            return Err(PathPrefixError::Ambiguous);
        }
        Ok(Self(if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PathPrefix {
    type Error = PathPrefixError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for PathPrefix {
    type Err = PathPrefixError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<PathPrefix> for String {
    fn from(prefix: PathPrefix) -> Self {
        prefix.0
    }
}

impl fmt::Display for PathPrefix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
