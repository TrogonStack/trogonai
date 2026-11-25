use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::MountPathError;

/// The absolute directory a sandbox mounts a drive at.
///
/// It starts with `/`, does not resolve to `/` (such as `/.` or `//`), and has no `..` component.
/// Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MountPath(String);

impl MountPath {
    pub fn new(value: impl Into<String>) -> Result<Self, MountPathError> {
        let value = value.into();
        if !value.starts_with('/') {
            return Err(MountPathError::NotAbsolute);
        }
        if value
            .split('/')
            .all(|component| component.is_empty() || component == ".")
        {
            return Err(MountPathError::Root);
        }
        if value.split('/').any(|component| component == "..") {
            return Err(MountPathError::ParentComponent);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MountPath {
    type Error = MountPathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for MountPath {
    type Err = MountPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<MountPath> for String {
    fn from(value: MountPath) -> Self {
        value.0
    }
}

impl fmt::Display for MountPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
