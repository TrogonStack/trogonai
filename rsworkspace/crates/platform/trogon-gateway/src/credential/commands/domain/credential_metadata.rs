use super::{CredentialFingerprint, CredentialRef, CredentialStatus, StorageLocator};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialMetadata {
    reference: CredentialRef,
    status: CredentialStatus,
    storage_locator: StorageLocator,
    fingerprint: CredentialFingerprint,
}

impl CredentialMetadata {
    pub fn new(
        reference: CredentialRef,
        status: CredentialStatus,
        storage_locator: StorageLocator,
        fingerprint: CredentialFingerprint,
    ) -> Self {
        Self {
            reference,
            status,
            storage_locator,
            fingerprint,
        }
    }

    pub fn reference(&self) -> &CredentialRef {
        &self.reference
    }

    pub fn status(&self) -> CredentialStatus {
        self.status
    }

    pub fn storage_locator(&self) -> &StorageLocator {
        &self.storage_locator
    }

    pub fn fingerprint(&self) -> &CredentialFingerprint {
        &self.fingerprint
    }
}

#[cfg(test)]
mod tests;
