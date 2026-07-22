use std::{borrow::Borrow, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable schema type name for a snapshot payload.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotTypeName(String);

impl SnapshotTypeName {
    /// Creates a snapshot type name after rejecting invalid input.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidSnapshotTypeName> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidSnapshotTypeName::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(InvalidSnapshotTypeName::ContainsControlCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the snapshot type name as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SnapshotTypeName {
    type Err = InvalidSnapshotTypeName;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for SnapshotTypeName {
    type Error = InvalidSnapshotTypeName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for SnapshotTypeName {
    type Error = InvalidSnapshotTypeName;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for SnapshotTypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for SnapshotTypeName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for SnapshotTypeName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Serialize for SnapshotTypeName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SnapshotTypeName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Error returned when constructing an invalid [`SnapshotTypeName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSnapshotTypeName {
    /// Snapshot type names cannot be empty.
    #[error("snapshot type name cannot be empty")]
    Empty,
    /// Snapshot type names cannot contain control characters.
    #[error("snapshot type name cannot contain control characters")]
    ContainsControlCharacter,
}

/// Associates a decider state type with the stable [`SnapshotTypeName`] its
/// snapshots are tagged with, so a decoder can reject a snapshot payload
/// encoded for a different state type.
pub trait SnapshotType {
    /// Error returned when the type name cannot be resolved or constructed.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Returns the stable type name for this state's snapshots.
    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error>;
}

#[cfg(test)]
mod tests;
