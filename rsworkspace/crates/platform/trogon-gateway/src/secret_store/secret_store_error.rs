use crate::credential::commands::domain::{
    CredentialFingerprintError, CredentialRef, CredentialStatus, StorageBackend,
};

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
}

impl SecretStoreError {
    pub(super) fn invalid_fingerprint(backend: StorageBackend) -> impl FnOnce(CredentialFingerprintError) -> Self {
        move |error| Self::invalid_generated(backend, "credential fingerprint", error)
    }

    pub(super) fn invalid_generated(backend: StorageBackend, what: &str, error: impl std::fmt::Display) -> Self {
        Self::BackendUnavailable {
            backend,
            message: format!("generated {what} is invalid: {error}"),
        }
    }
}

#[cfg(test)]
mod tests;
