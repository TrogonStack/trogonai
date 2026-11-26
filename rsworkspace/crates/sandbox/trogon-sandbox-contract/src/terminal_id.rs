use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::TerminalIdError;

/// The caller-chosen identifier of an interactive terminal.
///
/// It becomes part of a message subject, so it holds 1 to [`TerminalId::MAX_LEN`] bytes of ASCII
/// letters, digits, `-`, or `_`. Callers should pick an unguessable value because the id addresses the
/// terminal's output. Construction, parsing, and deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TerminalId(String);

impl TerminalId {
    /// Longest terminal id the wire accepts, in bytes.
    pub const MAX_LEN: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, TerminalIdError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_LEN {
            return Err(TerminalIdError::InvalidLength { max: Self::MAX_LEN });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(TerminalIdError::InvalidCharacters);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TerminalId {
    type Error = TerminalIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for TerminalId {
    type Err = TerminalIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<TerminalId> for String {
    fn from(value: TerminalId) -> Self {
        value.0
    }
}

impl fmt::Display for TerminalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
