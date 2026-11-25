use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::SnapshotTagNameError;

/// The name of a snapshot tag within its tenant.
///
/// It holds 1 to [`SnapshotTagName::MAX_LEN`] bytes of ASCII letters, digits, `.`, `_`, or `-`.
/// Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SnapshotTagName(String);

impl SnapshotTagName {
    /// Longest tag name the wire accepts, in bytes.
    pub const MAX_LEN: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, SnapshotTagNameError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_LEN {
            return Err(SnapshotTagNameError::InvalidLength);
        }
        if !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
        {
            return Err(SnapshotTagNameError::InvalidCharacters);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SnapshotTagName {
    type Error = SnapshotTagNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for SnapshotTagName {
    type Err = SnapshotTagNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<SnapshotTagName> for String {
    fn from(value: SnapshotTagName) -> Self {
        value.0
    }
}

impl fmt::Display for SnapshotTagName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
