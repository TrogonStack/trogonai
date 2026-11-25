use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::NativeSnapshotIdError;

/// A provider's own identifier for a snapshot it keeps.
///
/// It holds 1 to [`NativeSnapshotId::MAX_LEN`] bytes of ASCII letters, digits, `-`, `_`, or `.`.
/// Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NativeSnapshotId(String);

impl NativeSnapshotId {
    /// Longest provider snapshot id the wire accepts, in bytes.
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> Result<Self, NativeSnapshotIdError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_LEN {
            return Err(NativeSnapshotIdError::InvalidLength { max: Self::MAX_LEN });
        }
        if !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        {
            return Err(NativeSnapshotIdError::InvalidCharacters);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NativeSnapshotId {
    type Error = NativeSnapshotIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for NativeSnapshotId {
    type Err = NativeSnapshotIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<NativeSnapshotId> for String {
    fn from(value: NativeSnapshotId) -> Self {
        value.0
    }
}

impl fmt::Display for NativeSnapshotId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
