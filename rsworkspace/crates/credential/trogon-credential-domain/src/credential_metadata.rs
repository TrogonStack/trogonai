use super::{CredentialFingerprint, CredentialRef, CredentialStatus};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialMetadata {
    reference: CredentialRef,
    status: CredentialStatus,
    fingerprint: CredentialFingerprint,
}

impl CredentialMetadata {
    pub fn new(reference: CredentialRef, status: CredentialStatus, fingerprint: CredentialFingerprint) -> Self {
        Self {
            reference,
            status,
            fingerprint,
        }
    }

    pub fn reference(&self) -> &CredentialRef {
        &self.reference
    }

    pub fn status(&self) -> CredentialStatus {
        self.status
    }

    pub fn fingerprint(&self) -> &CredentialFingerprint {
        &self.fingerprint
    }
}

#[cfg(test)]
mod tests;
