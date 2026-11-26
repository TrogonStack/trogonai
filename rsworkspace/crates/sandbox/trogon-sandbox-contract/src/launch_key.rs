use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::LaunchKeyError;

/// A caller-chosen key that makes a retried command launch safe.
///
/// When the broker already saw the key for a lease, it replays the first launch's response instead of running the
/// command again, so a retry after a lost reply never runs a non-idempotent command twice. Keys are scoped to the
/// lease.
///
/// A key is 1 to [`LaunchKey::MAX_LEN`] characters of `[A-Za-z0-9_-]`, so it can sit in a key-value key or a subject
/// token without escaping. Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LaunchKey(String);

impl LaunchKey {
    /// Longest key accepted, in characters.
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> Result<Self, LaunchKeyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(LaunchKeyError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(LaunchKeyError::TooLong { max: Self::MAX_LEN });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(LaunchKeyError::InvalidCharacters);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LaunchKey {
    type Error = LaunchKeyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for LaunchKey {
    type Err = LaunchKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<LaunchKey> for String {
    fn from(value: LaunchKey) -> Self {
        value.0
    }
}

impl fmt::Display for LaunchKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
