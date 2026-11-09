use ed25519_dalek::VerifyingKey;

use crate::{RegistrationError, Sha256Digest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalKey(VerifyingKey);

impl ApprovalKey {
    pub fn new(key: VerifyingKey) -> Result<Self, RegistrationError> {
        if key.is_weak() {
            return Err(RegistrationError::WeakApprovalKey);
        }
        Ok(Self(key))
    }

    pub fn fingerprint(&self) -> Sha256Digest {
        Sha256Digest::hash(self.0.as_bytes())
    }

    pub(crate) fn verifying_key(&self) -> &VerifyingKey {
        &self.0
    }
}
