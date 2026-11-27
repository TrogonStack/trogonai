use std::fmt;

use serde::{Deserialize, Serialize};

use crate::CommitShaError;

/// A hexadecimal commit identifier of 4 to 64 characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CommitSha(String);

impl CommitSha {
    /// The all-zero SHA a workspace that is not a clone carries.
    pub fn placeholder() -> Self {
        Self("0".repeat(40))
    }

    pub fn new(value: impl Into<String>) -> Result<Self, CommitShaError> {
        let value = value.into();
        let len = value.len();
        if !(4..=64).contains(&len) {
            return Err(CommitShaError::InvalidLength(len));
        }
        if !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CommitShaError::NonHex);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CommitSha {
    type Error = CommitShaError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<CommitSha> for String {
    fn from(value: CommitSha) -> Self {
        value.0
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
