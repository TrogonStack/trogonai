use std::fmt;
use std::sync::Arc;

const MAX_FINGERPRINT_LEN: usize = 128;

#[derive(Clone, Eq, PartialEq)]
pub struct CredentialFingerprint(Arc<str>);

impl CredentialFingerprint {
    pub fn new(value: impl AsRef<str>) -> Result<Self, CredentialFingerprintError> {
        let value = value.as_ref();
        let char_count = value.chars().count();
        if char_count == 0 {
            return Err(CredentialFingerprintError::Empty);
        }
        if char_count > MAX_FINGERPRINT_LEN {
            return Err(CredentialFingerprintError::TooLong(char_count));
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialFingerprint").field(&self.as_str()).finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialFingerprintError {
    #[error("credential fingerprint must not be empty")]
    Empty,
    #[error("credential fingerprint exceeds maximum length: {0}")]
    TooLong(usize),
}
