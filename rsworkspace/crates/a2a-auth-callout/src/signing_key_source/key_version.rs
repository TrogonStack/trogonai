use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyVersion(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyVersionError {
    Empty,
}

impl fmt::Display for KeyVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("key version must be non-empty"),
        }
    }
}

impl std::error::Error for KeyVersionError {}

impl KeyVersion {
    pub fn new(version: impl Into<String>) -> Result<Self, KeyVersionError> {
        let s = version.into();
        if s.is_empty() {
            return Err(KeyVersionError::Empty);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) fn unminted_placeholder() -> KeyVersion {
    KeyVersion::new("pending").expect("static placeholder")
}
