use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::SnapshotGenerationError;

/// Identifies one publication of a snapshot so accounting can tell a retry from a newer capture.
///
/// It is a canonical ULID: [`SnapshotGeneration::LEN`] uppercase Crockford base32 characters whose
/// first character is `0` through `7`, so the value fits in 128 bits. It serializes as a JSON
/// string. Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SnapshotGeneration(String);

impl SnapshotGeneration {
    /// Length of a canonical ULID, in characters.
    pub const LEN: usize = 26;

    const ALPHABET: &'static [u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

    pub fn new(value: impl Into<String>) -> Result<Self, SnapshotGenerationError> {
        let value = value.into();
        if value.len() != Self::LEN {
            return Err(SnapshotGenerationError::Length { expected: Self::LEN });
        }
        if !value.bytes().all(|byte| Self::ALPHABET.contains(&byte)) {
            return Err(SnapshotGenerationError::InvalidCharacter);
        }
        if !matches!(value.as_bytes().first(), Some(b'0'..=b'7')) {
            return Err(SnapshotGenerationError::Overflow);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SnapshotGeneration {
    type Error = SnapshotGenerationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for SnapshotGeneration {
    type Err = SnapshotGenerationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<SnapshotGeneration> for String {
    fn from(value: SnapshotGeneration) -> Self {
        value.0
    }
}

impl fmt::Display for SnapshotGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
