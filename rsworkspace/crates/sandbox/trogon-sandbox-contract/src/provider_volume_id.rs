use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ProviderVolumeIdError;

/// A provider's own identifier for a storage volume.
///
/// It holds 1 to [`ProviderVolumeId::MAX_LEN`] bytes with no whitespace or control characters.
/// Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderVolumeId(String);

impl ProviderVolumeId {
    /// Longest provider volume identifier the wire accepts, in bytes.
    pub const MAX_LEN: usize = 256;

    pub fn new(value: impl Into<String>) -> Result<Self, ProviderVolumeIdError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > Self::MAX_LEN
            || value
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(ProviderVolumeIdError::Invalid);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProviderVolumeId {
    type Error = ProviderVolumeIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ProviderVolumeId {
    type Err = ProviderVolumeIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<ProviderVolumeId> for String {
    fn from(value: ProviderVolumeId) -> Self {
        value.0
    }
}

impl fmt::Display for ProviderVolumeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
