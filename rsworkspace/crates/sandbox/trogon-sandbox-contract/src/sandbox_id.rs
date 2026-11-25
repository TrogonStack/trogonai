use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::SandboxIdError;

/// A provider's identifier for a running sandbox.
///
/// It holds 1 to [`SandboxId::MAX_LEN`] bytes with no control characters and serializes as a JSON string.
/// Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SandboxId(String);

impl SandboxId {
    /// Longest sandbox identifier the wire accepts, in bytes.
    pub const MAX_LEN: usize = 512;

    pub fn new(value: impl Into<String>) -> Result<Self, SandboxIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SandboxIdError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(SandboxIdError::TooLong { max: Self::MAX_LEN });
        }
        if value.chars().any(char::is_control) {
            return Err(SandboxIdError::ControlCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SandboxId {
    type Error = SandboxIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for SandboxId {
    type Err = SandboxIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<SandboxId> for String {
    fn from(value: SandboxId) -> Self {
        value.0
    }
}

impl fmt::Display for SandboxId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
