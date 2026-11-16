use trogon_credential_domain::CredentialFingerprintError;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GeneratedValueError {
    #[error("generated credential fingerprint is invalid: {0}")]
    CredentialFingerprint(#[source] CredentialFingerprintError),
}

impl From<CredentialFingerprintError> for GeneratedValueError {
    fn from(error: CredentialFingerprintError) -> Self {
        Self::CredentialFingerprint(error)
    }
}
