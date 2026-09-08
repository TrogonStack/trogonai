use std::fmt;

use serde::{Deserialize, Serialize};

use crate::constants::{VERSION_CURRENT, VERSION_PREVIOUS};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyVersion(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyVersionError {
    #[error("key version must be non-empty")]
    Empty,
}

impl KeyVersion {
    pub(crate) fn current() -> Self {
        Self(VERSION_CURRENT.to_owned())
    }

    pub(crate) fn previous() -> Self {
        Self(VERSION_PREVIOUS.to_owned())
    }

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
    KeyVersion("pending".to_owned())
}
