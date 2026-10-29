use super::*;

#[test]
fn invalid_generated_names_the_backend_and_the_offending_value() {
    let error = SecretStoreError::invalid_generated(StorageBackend::InMemory, "credential id", "too long");

    assert_eq!(
        error,
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::InMemory,
            message: "generated credential id is invalid: too long".to_string(),
        }
    );
    assert_eq!(
        error.to_string(),
        "in_memory secret store error: generated credential id is invalid: too long"
    );
}

#[test]
fn invalid_fingerprint_reports_the_generated_fingerprint() {
    let error = SecretStoreError::invalid_fingerprint(StorageBackend::OpenBao)(CredentialFingerprintError::Empty);

    assert_eq!(
        error,
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            message: "generated credential fingerprint is invalid: credential fingerprint must not be empty"
                .to_string(),
        }
    );
}
