use crate::credential::commands::domain::{CredentialFingerprintError, StorageLocatorError};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GeneratedValueError {
    #[error("generated storage locator is invalid: {0}")]
    StorageLocator(#[source] StorageLocatorError),
    #[error("generated credential fingerprint is invalid: {0}")]
    CredentialFingerprint(#[source] CredentialFingerprintError),
}

impl From<StorageLocatorError> for GeneratedValueError {
    fn from(error: StorageLocatorError) -> Self {
        Self::StorageLocator(error)
    }
}

impl From<CredentialFingerprintError> for GeneratedValueError {
    fn from(error: CredentialFingerprintError) -> Self {
        Self::CredentialFingerprint(error)
    }
}
