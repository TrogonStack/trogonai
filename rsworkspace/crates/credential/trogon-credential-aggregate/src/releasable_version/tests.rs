use buffa::MessageField;
use trogon_credential_domain::{
    CredentialDestroyReason, CredentialFailureReason, CredentialFingerprint, CredentialIdentity, CredentialKind,
    CredentialOwnerId, CredentialScope, CredentialVersion, CredentialVersionError, SourceKind,
};
use trogonai_proto::gateway::credentials::CredentialStateSnapshotCase;

use super::*;
use crate::CredentialFieldInvalidError;
use crate::proto::{
    active_state_to_proto, cleanup_failed_to_proto_state, destroy_requested_to_proto_state, destroyed_to_proto_state,
    pending_write_to_proto_state, revoked_to_proto_state, rotation_pending_to_proto_state, write_failed_to_proto_state,
};

fn identity() -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
}

fn credential_ref(value: u64) -> CredentialRef {
    CredentialRef::new(identity(), CredentialVersion::new(value).unwrap())
}

fn fingerprint() -> CredentialFingerprint {
    CredentialFingerprint::new("fingerprint").unwrap()
}

fn failure() -> CredentialFailureReason {
    CredentialFailureReason::new("openbao failed").unwrap()
}

fn destroy_reason() -> CredentialDestroyReason {
    CredentialDestroyReason::new("cleanup").unwrap()
}

fn snapshot(state: impl Into<CredentialStateSnapshotCase>) -> state_v1::CredentialStateSnapshot {
    state_v1::CredentialStateSnapshot {
        state: Some(state.into()),
    }
}

fn active() -> state_v1::ActiveCredentialState {
    active_state_to_proto(
        &credential_ref(2),
        &fingerprint(),
        &[CredentialVersion::new(1).unwrap()],
    )
}

#[test]
fn an_active_credential_releases_its_current_version() {
    assert_eq!(releasable_version(&snapshot(active())), Ok(Some(credential_ref(2))));
}

#[test]
fn a_pending_rotation_releases_the_version_it_would_replace() {
    let state = snapshot(rotation_pending_to_proto_state(active()));

    assert_eq!(releasable_version(&state), Ok(Some(credential_ref(2))));
}

#[test]
fn a_missing_credential_releases_nothing() {
    let missing = snapshot(state_v1::CredentialMissingState {});
    let empty = state_v1::CredentialStateSnapshot { state: None };

    assert_eq!(releasable_version(&missing), Ok(None));
    assert_eq!(releasable_version(&empty), Ok(None));
}

#[test]
fn a_pending_write_releases_nothing() {
    let state = snapshot(pending_write_to_proto_state(&identity()));

    assert_eq!(releasable_version(&state), Ok(None));
}

#[test]
fn a_failed_write_releases_nothing() {
    let state = snapshot(write_failed_to_proto_state(&identity(), &failure()));

    assert_eq!(releasable_version(&state), Ok(None));
}

#[test]
fn a_revoked_credential_releases_nothing() {
    let state = snapshot(revoked_to_proto_state(&credential_ref(2)));

    assert_eq!(releasable_version(&state), Ok(None));
}

#[test]
fn a_credential_being_destroyed_releases_nothing() {
    let state = snapshot(destroy_requested_to_proto_state(&credential_ref(2), &destroy_reason()));

    assert_eq!(releasable_version(&state), Ok(None));
}

#[test]
fn a_destroyed_credential_releases_nothing() {
    let state = snapshot(destroyed_to_proto_state(&credential_ref(2)));

    assert_eq!(releasable_version(&state), Ok(None));
}

#[test]
fn a_credential_whose_cleanup_failed_releases_nothing() {
    let state = snapshot(cleanup_failed_to_proto_state(&credential_ref(2), &failure()));

    assert_eq!(releasable_version(&state), Ok(None));
}

#[test]
fn a_releasable_state_that_does_not_decode_is_an_error() {
    let zero_version = state_v1::ActiveCredentialState {
        version: Some(0),
        ..active()
    };
    let rotation_without_active = state_v1::RotationPendingCredentialState {
        active: MessageField::none(),
    };

    assert_eq!(
        releasable_version(&snapshot(zero_version.clone())),
        Err(CredentialProtoDecodeError::InvalidField {
            field: "active.version".into(),
            reason: CredentialFieldInvalidError::Version(CredentialVersionError::Zero),
        })
    );
    assert_eq!(
        releasable_version(&snapshot(rotation_pending_to_proto_state(zero_version))),
        Err(CredentialProtoDecodeError::InvalidField {
            field: "active.version".into(),
            reason: CredentialFieldInvalidError::Version(CredentialVersionError::Zero),
        })
    );
    assert_eq!(
        releasable_version(&snapshot(rotation_without_active)),
        Err(CredentialProtoDecodeError::MissingField {
            field: "rotation_pending.active".into(),
        })
    );
}
