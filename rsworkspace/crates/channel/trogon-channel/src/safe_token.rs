//! A single token safe for channel KV keys and NATS subject tails.
//!
//! Tokens are joined with `.` into composite keys, so `.` is out; the rest is
//! the intersection of what NATS KV keys and NATS subject tokens accept.

#[cfg(test)]
mod tests;

use serde::{Deserialize, Deserializer, Serialize};

/// Why a candidate token failed [`SafeToken`] construction.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SafeTokenError {
    #[error("token must not be empty")]
    Empty,
    #[error("token contains invalid character: {0:?}")]
    InvalidCharacter(char),
}

/// One NATS/KV-safe token. Validity is guaranteed at construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SafeToken(String);

impl SafeToken {
    pub fn new(token: impl Into<String>) -> Result<Self, SafeTokenError> {
        let token = token.into();
        if token.is_empty() {
            return Err(SafeTokenError::Empty);
        }
        if let Some(c) = token
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=')))
        {
            return Err(SafeTokenError::InvalidCharacter(c));
        }
        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Platforms number their users, chats, and messages, and a decimal integer is
/// a token by construction: its digits are all in the allowed set. Converting
/// through [`SafeToken::new`] instead would leave every caller on that path with
/// an error arm nothing can reach.
impl From<u64> for SafeToken {
    fn from(value: u64) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Display for SafeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SafeToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}
