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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidSnapshotTypeName {
    /// Snapshot type names cannot be empty.
    Empty,
    /// Snapshot type names cannot contain control characters.
    ContainsControlCharacter,
}

impl std::fmt::Display for InvalidSnapshotTypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("snapshot type name cannot be empty"),
            Self::ContainsControlCharacter => f.write_str("snapshot type name cannot contain control characters"),
        }
    }
}

impl std::error::Error for InvalidSnapshotTypeName {}

pub trait SnapshotType {
    type Error: std::error::Error + Send + Sync + 'static;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_type_name_rejects_empty_value() {
        assert_eq!(SnapshotTypeName::new("").unwrap_err(), InvalidSnapshotTypeName::Empty);
    }

    #[test]
    fn snapshot_type_name_rejects_control_characters() {
        assert_eq!(
            SnapshotTypeName::new("test.snapshot\nv1").unwrap_err(),
            InvalidSnapshotTypeName::ContainsControlCharacter
        );
    }

    #[test]
    fn snapshot_type_name_serializes_as_string() {
        let snapshot_type = SnapshotTypeName::new("test.snapshot.v1").unwrap();

        assert_eq!(serde_json::to_string(&snapshot_type).unwrap(), "\"test.snapshot.v1\"");
    }

    #[test]
    fn snapshot_type_name_deserializes_with_validation() {
        let snapshot_type: SnapshotTypeName = serde_json::from_str("\"test.snapshot.v1\"").unwrap();

        assert_eq!(snapshot_type.as_str(), "test.snapshot.v1");
        assert!(serde_json::from_str::<SnapshotTypeName>("\"\"").is_err());
    }

    #[test]
    fn snapshot_type_name_supports_string_conversions_and_views() {
        let parsed = "test.snapshot.v1".parse::<SnapshotTypeName>().unwrap();
        let from_string = SnapshotTypeName::try_from("test.snapshot.v1".to_string()).unwrap();
        let from_str = SnapshotTypeName::try_from("test.snapshot.v1").unwrap();

        assert_eq!(parsed, from_string);
        assert_eq!(from_str.as_ref(), "test.snapshot.v1");
        assert_eq!(std::borrow::Borrow::<str>::borrow(&parsed), "test.snapshot.v1");
        assert_eq!(parsed.to_string(), "test.snapshot.v1");
    }

    #[test]
    fn invalid_snapshot_type_name_displays_reason() {
        assert_eq!(
            InvalidSnapshotTypeName::Empty.to_string(),
            "snapshot type name cannot be empty"
        );
        assert_eq!(
            InvalidSnapshotTypeName::ContainsControlCharacter.to_string(),
            "snapshot type name cannot contain control characters"
        );
    }
}
