use trogon_credential_domain::{CredentialRef, CredentialStatus};

use super::GeneratedValueError;

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
    #[error("secret store error: {message}")]
    BackendUnavailable { message: String },
    #[error("secret store error: {source}")]
    InvalidGenerated {
        #[source]
        source: GeneratedValueError,
    },
}

impl SecretStoreError {
    pub(super) fn invalid_generated(error: impl Into<GeneratedValueError>) -> Self {
        Self::InvalidGenerated { source: error.into() }
    }
}

#[cfg(test)]
mod tests;
