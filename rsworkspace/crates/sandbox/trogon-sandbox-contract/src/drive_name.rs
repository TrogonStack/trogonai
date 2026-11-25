use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::DriveNameError;

/// The name of a persistent drive within its tenant.
///
/// It holds 1 to [`DriveName::MAX_LEN`] bytes without `/`, `\`, or `..`. Construction, parsing, and
/// deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DriveName(String);

impl DriveName {
    /// Longest drive name the wire accepts, in bytes.
    pub const MAX_LEN: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, DriveNameError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_LEN {
            return Err(DriveNameError::InvalidLength);
        }
        if value.contains('/') || value.contains('\\') || value.contains("..") {
            return Err(DriveNameError::InvalidCharacters);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DriveName {
    type Error = DriveNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for DriveName {
    type Err = DriveNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<DriveName> for String {
    fn from(value: DriveName) -> Self {
        value.0
    }
}

impl fmt::Display for DriveName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
