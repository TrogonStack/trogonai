use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::SshSessionIdError;

/// The identifier of one gateway session.
///
/// It becomes a NATS subject token and part of a terminal id, so it holds 1 to [`SshSessionId::MAX_LEN`] bytes of
/// ASCII letters, digits, `-`, or `_`. Construction, parsing, and deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SshSessionId(String);

impl SshSessionId {
    /// Longest session id the wire accepts, in bytes.
    pub const MAX_LEN: usize = 48;

    pub fn new(value: impl Into<String>) -> Result<Self, SshSessionIdError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_LEN {
            return Err(SshSessionIdError::InvalidLength { max: Self::MAX_LEN });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(SshSessionIdError::InvalidCharacters);
        }
        Ok(Self(value))
    }

    /// A fresh random session id.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SshSessionId {
    type Error = SshSessionIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for SshSessionId {
    type Err = SshSessionIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<SshSessionId> for String {
    fn from(value: SshSessionId) -> Self {
        value.0
    }
}

impl fmt::Display for SshSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
