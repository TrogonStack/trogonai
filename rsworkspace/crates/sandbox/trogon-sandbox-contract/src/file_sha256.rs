use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::FileSha256Error;

/// A SHA-256 checksum of file content, written as exactly 64 lowercase hexadecimal characters
/// with no prefix.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FileSha256(String);

impl FileSha256 {
    /// Length of the hexadecimal text, in characters.
    pub const HEX_LEN: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, FileSha256Error> {
        let value = value.into();
        if value.len() != Self::HEX_LEN {
            return Err(FileSha256Error::Length {
                expected: Self::HEX_LEN,
            });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(FileSha256Error::NotLowercaseHex);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for FileSha256 {
    type Err = FileSha256Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for FileSha256 {
    type Error = FileSha256Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<FileSha256> for String {
    fn from(value: FileSha256) -> Self {
        value.0
    }
}

impl fmt::Display for FileSha256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
