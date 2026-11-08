use std::fmt;
use std::sync::Arc;

use super::constants::MAX_STORAGE_LOCATOR_LEN;
use super::{CredentialRef, StorageBackend, StorageBackendError};

/// Where an adapter put a credential's material.
///
/// Recorded in activation events and aggregate state separately from the
/// opaque credential identity. For OpenBao it is the leaf of the ADR#0046
/// path. It is internal storage metadata, absent from the public credential
/// metadata contract.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StorageLocator {
    value: Arc<str>,
    backend: StorageBackend,
}

impl StorageLocator {
    pub fn new(value: impl AsRef<str>) -> Result<Self, StorageLocatorError> {
        let value = value.as_ref();
        let char_count = value.chars().count();
        if char_count == 0 {
            return Err(StorageLocatorError::Empty);
        }
        if char_count > MAX_STORAGE_LOCATOR_LEN {
            return Err(StorageLocatorError::TooLong(char_count));
        }
        for ch in value.chars() {
            if !is_storage_locator_char(ch) {
                return Err(StorageLocatorError::InvalidCharacter(ch));
            }
        }
        if value.split('/').any(|segment| matches!(segment, "." | "..")) {
            return Err(StorageLocatorError::DotSegment);
        }
        let backend = StorageBackend::from_locator(value)?;
        Ok(Self {
            value: Arc::from(value),
            backend,
        })
    }

    /// The leaf of the ADR#0046 path an OpenBao credential is written under.
    pub fn openbao(credential: &CredentialRef) -> Result<Self, StorageLocatorError> {
        Self::new(format!(
            "openbao:{}:{}:{}",
            credential.owner_id(),
            credential.scope_key(),
            credential.kind().as_str()
        ))
    }

    pub fn in_memory(credential: &CredentialRef) -> Result<Self, StorageLocatorError> {
        Self::new(format!(
            "memory:{}:{}:{}",
            credential.owner_id(),
            credential.scope_key(),
            credential.kind().as_str()
        ))
    }

    pub fn static_config(credential: &CredentialRef) -> Result<Self, StorageLocatorError> {
        Self::new(format!(
            "static-config:{}:{}",
            credential.scope_key(),
            credential.kind().as_str()
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn backend(&self) -> StorageBackend {
        self.backend
    }
}

impl fmt::Display for StorageLocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StorageLocatorError {
    #[error("storage locator must not be empty")]
    Empty,
    #[error("storage locator contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("storage locator exceeds maximum length: {0}")]
    TooLong(usize),
    #[error("storage locator must not contain a dot path segment")]
    DotSegment,
    #[error(transparent)]
    Backend(#[from] StorageBackendError),
}

fn is_storage_locator_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/')
}

#[cfg(test)]
mod tests;
