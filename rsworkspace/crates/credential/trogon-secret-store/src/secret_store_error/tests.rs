use trogon_credential_domain::CredentialFingerprintError;

use super::*;

#[test]
fn invalid_generated_keeps_the_fingerprint_error_as_its_source() {
    let error = SecretStoreError::invalid_generated(CredentialFingerprintError::Empty);

    assert_eq!(
        error,
        SecretStoreError::InvalidGenerated {
            source: GeneratedValueError::CredentialFingerprint(CredentialFingerprintError::Empty),
        }
    );
    assert_eq!(
        error.to_string(),
        "secret store error: generated credential fingerprint is invalid: credential fingerprint must not be empty"
    );
    assert!(std::error::Error::source(&error).is_some());
}
