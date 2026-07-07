use super::{CredentialRef, CredentialStatus, StorageBackend};

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
