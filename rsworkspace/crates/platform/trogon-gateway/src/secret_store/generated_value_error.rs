use crate::credential::commands::domain::{CredentialFingerprintError, CredentialIdError};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GeneratedValueError {
    #[error("generated credential id is invalid: {0}")]
    CredentialId(#[source] CredentialIdError),
    #[error("generated credential fingerprint is invalid: {0}")]
    CredentialFingerprint(#[source] CredentialFingerprintError),
}

impl From<CredentialIdError> for GeneratedValueError {
    fn from(error: CredentialIdError) -> Self {
        Self::CredentialId(error)
    }
}

impl From<CredentialFingerprintError> for GeneratedValueError {
    fn from(error: CredentialFingerprintError) -> Self {
        Self::CredentialFingerprint(error)
    }
}
