use super::*;
use crate::credential::commands::domain::{CredentialFingerprintError, CredentialIdError};

#[test]
fn invalid_generated_keeps_the_credential_id_error_as_its_source() {
    let error = SecretStoreError::invalid_generated(StorageBackend::InMemory)(CredentialIdError::Empty);

    assert_eq!(
        error,
        SecretStoreError::InvalidGenerated {
            backend: StorageBackend::InMemory,
            source: GeneratedValueError::CredentialId(CredentialIdError::Empty),
        }
    );
    assert_eq!(
        error.to_string(),
        "in_memory secret store error: generated credential id is invalid: credential id must not be empty"
    );
}

#[test]
fn invalid_generated_keeps_the_fingerprint_error_as_its_source() {
    let error = SecretStoreError::invalid_generated(StorageBackend::OpenBao)(CredentialFingerprintError::Empty);

    assert_eq!(
        error.to_string(),
        "openbao secret store error: generated credential fingerprint is invalid: credential fingerprint must not be empty"
    );
    assert!(std::error::Error::source(&error).is_some());
}
