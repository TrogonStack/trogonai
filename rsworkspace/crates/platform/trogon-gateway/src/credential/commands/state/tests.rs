use trogon_credential_domain::{
    CredentialFingerprint, CredentialIdentityVersion, CredentialKind, CredentialOwnerId, CredentialScope, SourceKind,
};

use super::*;

fn scope() -> CredentialScope {
    CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::GitHub)
}

fn identity() -> CredentialIdentity {
    CredentialIdentity::derive(scope(), CredentialKind::WebhookSecret)
}

/// The stream every event in these tests is read from.
fn stream() -> CredentialKey {
    identity().key()
}

/// The recorded id and owner under another slot: the same key, a different identity.
fn identity_of_another_slot() -> CredentialIdentity {
    CredentialIdentity::new(
        identity().id().clone(),
        scope(),
        CredentialKind::SigningSecret,
        CredentialIdentityVersion::V1,
    )
}

fn version(value: u64) -> CredentialVersion {
    CredentialVersion::new(value).unwrap()
}

fn credential_ref(value: u64) -> CredentialRef {
    CredentialRef::new(identity(), version(value))
}

fn fingerprint() -> CredentialFingerprint {
    CredentialFingerprint::new("fingerprint").unwrap()
}

fn active_metadata(credential: CredentialRef) -> CredentialMetadata {
    CredentialMetadata::new(credential, CredentialStatus::Active, fingerprint())
}

#[test]
fn decide_holds_a_command_to_the_identity_and_version_its_stream_recorded() {
    let another_slot = CredentialRef::new(identity_of_another_slot(), version(1));

    assert_eq!(ensure_recorded_identity(&identity(), &identity()), Ok(()));
    assert_eq!(
        ensure_recorded_identity(&identity(), another_slot.identity()),
        Err(CredentialDecideError::IdentityMismatch { key: stream() })
    );
    assert_eq!(ensure_recorded_ref(&credential_ref(1), &credential_ref(1)), Ok(()));
    assert_eq!(
        ensure_recorded_ref(&credential_ref(1), &another_slot),
        Err(CredentialDecideError::IdentityMismatch { key: stream() })
    );
    assert_eq!(
        ensure_recorded_ref(&credential_ref(1), &credential_ref(2)),
        Err(CredentialDecideError::VersionMismatch {
            expected: version(1),
            actual: version(2),
        })
    );
}

#[test]
fn decide_accepts_only_metadata_of_an_active_credential() {
    assert_eq!(ensure_active_metadata(&active_metadata(credential_ref(1))), Ok(()));
    assert_eq!(
        ensure_active_metadata(&CredentialMetadata::new(
            credential_ref(1),
            CredentialStatus::Pending,
            fingerprint()
        )),
        Err(CredentialDecideError::MetadataNotActive {
            status: CredentialStatus::Pending
        })
    );
}

#[test]
fn identity_errors_name_the_credentials_and_versions_involved() {
    let key = stream();
    let cases = [
        (
            CredentialDecideError::IdentityMismatch { key: key.clone() }.to_string(),
            format!("credential '{key}' was recorded with another identity"),
        ),
        (
            CredentialDecideError::VersionMismatch {
                expected: version(1),
                actual: version(2),
            }
            .to_string(),
            "credential version does not match the recorded version: expected 1, got 2".to_string(),
        ),
    ];

    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
}
