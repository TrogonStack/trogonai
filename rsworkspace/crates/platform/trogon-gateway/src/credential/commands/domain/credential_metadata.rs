use super::{CredentialFingerprint, CredentialRef, CredentialStatus, StorageBackend};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialMetadata {
    reference: CredentialRef,
    status: CredentialStatus,
    storage_backend: StorageBackend,
    fingerprint: CredentialFingerprint,
}

impl CredentialMetadata {
    pub fn new(
        reference: CredentialRef,
        status: CredentialStatus,
        storage_backend: StorageBackend,
        fingerprint: CredentialFingerprint,
    ) -> Self {
        Self {
            reference,
            status,
            storage_backend,
            fingerprint,
        }
    }

    pub fn reference(&self) -> &CredentialRef {
        &self.reference
    }

    pub fn status(&self) -> CredentialStatus {
        self.status
    }

    pub fn storage_backend(&self) -> StorageBackend {
        self.storage_backend
    }

    pub fn fingerprint(&self) -> &CredentialFingerprint {
        &self.fingerprint
    }
}
