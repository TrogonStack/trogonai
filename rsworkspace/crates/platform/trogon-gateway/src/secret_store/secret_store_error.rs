use super::GeneratedValueError;
use crate::credential::commands::domain::{CredentialRef, CredentialStatus, StorageBackend};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SecretStoreError {
    #[error("credential not found: {credential}")]
    Missing { credential: CredentialRef },
    #[error("credential is not readable: {credential} is {status}")]
    Unreadable {
        credential: CredentialRef,
        status: CredentialStatus,
    },
    #[error("credential is not writable: {credential} is {status}")]
    Unwritable {
        credential: CredentialRef,
        status: CredentialStatus,
    },
    #[error("{backend} secret store error: {message}")]
    BackendUnavailable { backend: StorageBackend, message: String },
    #[error("{backend} secret store error: {source}")]
    InvalidGenerated {
        backend: StorageBackend,
        #[source]
        source: GeneratedValueError,
    },
}

impl SecretStoreError {
    pub(super) fn invalid_generated<E: Into<GeneratedValueError>>(backend: StorageBackend) -> impl FnOnce(E) -> Self {
        move |error| Self::InvalidGenerated {
            backend,
            source: error.into(),
        }
    }
}

#[cfg(test)]
mod tests;
