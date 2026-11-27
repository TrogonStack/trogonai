use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::SandboxEntityIdError;

/// The random identifier of a persistent sandbox entity.
///
/// It is a ULID of [`SandboxEntityId::LEN`] Crockford base32 characters, never derived from the entity name, so it
/// cannot be guessed across tenants. Construction, parsing, and deserialization accept either case and store the
/// uppercase form. The letters `I`, `L`, `O`, and `U` are not part of the alphabet. It serializes as a JSON string.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SandboxEntityId(String);

impl SandboxEntityId {
    /// Length of an entity id, in characters.
    pub const LEN: usize = 26;

    pub fn new(value: impl Into<String>) -> Result<Self, SandboxEntityIdError> {
        let value = value.into().to_ascii_uppercase();
        if value.len() != Self::LEN {
            return Err(SandboxEntityIdError::Length { expected: Self::LEN });
        }
        let valid = value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (byte.is_ascii_uppercase() && !b"ILOU".contains(&byte)));
        if !valid {
            return Err(SandboxEntityIdError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SandboxEntityId {
    type Error = SandboxEntityIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for SandboxEntityId {
    type Err = SandboxEntityIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<SandboxEntityId> for String {
    fn from(value: SandboxEntityId) -> Self {
        value.0
    }
}

impl fmt::Display for SandboxEntityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
