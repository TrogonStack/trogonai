use buffa::MessageField;

use super::*;
use crate::credential::commands::domain::{
    CredentialDestroyReason, CredentialFailureReason, CredentialFingerprint, CredentialIdentityVersion, CredentialKind,
    CredentialOwnerId, CredentialScope, SourceKind,
};
use crate::credential::commands::proto::{
    activated_to_proto, destroy_failed_to_proto, destroy_requested_to_proto, destroyed_to_proto, revoked_to_proto,
    rotated_to_proto, rotation_failed_to_proto, rotation_requested_to_proto, write_failed_to_proto,
    write_requested_to_proto,
};

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

/// Another credential in the same project, on a stream of its own.
fn other_identity() -> CredentialIdentity {
    CredentialIdentity::derive(scope(), CredentialKind::SigningSecret)
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

fn failure() -> CredentialFailureReason {
    CredentialFailureReason::new("openbao failed").unwrap()
}

fn destroy_reason() -> CredentialDestroyReason {
    CredentialDestroyReason::new("cleanup").unwrap()
}

fn active_metadata(credential: CredentialRef) -> CredentialMetadata {
    CredentialMetadata::new(credential, CredentialStatus::Active, fingerprint())
}

fn event(event: impl Into<CredentialEventCase>) -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(event.into()),
    }
}

fn write_requested() -> v1::CredentialEvent {
    event(write_requested_to_proto(&identity()))
}

fn write_failed() -> v1::CredentialEvent {
    event(write_failed_to_proto(&stream(), &failure()))
}

fn activated() -> v1::CredentialEvent {
    event(activated_to_proto(&active_metadata(credential_ref(1))))
}

fn rotation_requested() -> v1::CredentialEvent {
    event(rotation_requested_to_proto(&credential_ref(1)))
}

fn rotation_failed() -> v1::CredentialEvent {
    event(rotation_failed_to_proto(&credential_ref(1), &failure()))
}

fn rotated() -> v1::CredentialEvent {
    event(rotated_to_proto(
        &credential_ref(1),
        &active_metadata(credential_ref(2)),
    ))
}

fn revoked() -> v1::CredentialEvent {
    event(revoked_to_proto(&credential_ref(1)))
}

fn destroy_requested() -> v1::CredentialEvent {
    event(destroy_requested_to_proto(&credential_ref(1), &destroy_reason()))
}

fn destroyed() -> v1::CredentialEvent {
    event(destroyed_to_proto(&credential_ref(1)))
}

fn destroy_failed() -> v1::CredentialEvent {
    event(destroy_failed_to_proto(&credential_ref(1), &failure()))
}

fn snapshot(state: impl Into<CredentialStateSnapshotCase>) -> state_v1::CredentialStateSnapshot {
    state_v1::CredentialStateSnapshot {
        state: Some(state.into()),
    }
}

fn state_after(events: impl IntoIterator<Item = v1::CredentialEvent>) -> state_v1::CredentialStateSnapshot {
    events
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve_stream(&stream(), state, &event))
        .unwrap()
}

fn pending() -> state_v1::CredentialStateSnapshot {
    state_after([write_requested()])
}

fn active() -> state_v1::CredentialStateSnapshot {
    state_after([write_requested(), activated()])
}

fn rotating() -> state_v1::CredentialStateSnapshot {
    state_after([write_requested(), activated(), rotation_requested()])
}

fn revoked_state() -> state_v1::CredentialStateSnapshot {
    state_after([write_requested(), activated(), revoked()])
}

fn destroying() -> state_v1::CredentialStateSnapshot {
    state_after([write_requested(), activated(), revoked(), destroy_requested()])
}

fn cleanup_failed() -> state_v1::CredentialStateSnapshot {
    state_after([
        write_requested(),
        activated(),
        revoked(),
        destroy_requested(),
        destroy_failed(),
    ])
}

fn destroyed_state() -> state_v1::CredentialStateSnapshot {
    state_after([
        write_requested(),
        activated(),
        revoked(),
        destroy_requested(),
        destroyed(),
    ])
}

fn rejected_by(state: state_v1::CredentialStateSnapshot, event: v1::CredentialEvent, expected: CredentialEvolveError) {
    assert_eq!(evolve_stream(&stream(), state, &event), Err(expected));
}

#[test]
fn every_state_after_the_write_request_holds_the_identity_it_recorded() {
    let cases = [
        (pending(), snapshot(pending_write_to_proto_state(&identity()))),
        (
            active(),
            snapshot(active_state_to_proto(&credential_ref(1), &fingerprint(), &[])),
        ),
        (
            rotating(),
            snapshot(rotation_pending_to_proto_state(active_state_to_proto(
                &credential_ref(1),
                &fingerprint(),
                &[],
            ))),
        ),
        (
            state_after([write_requested(), write_failed()]),
            snapshot(write_failed_to_proto_state(&identity(), &failure())),
        ),
        (revoked_state(), snapshot(revoked_to_proto_state(&credential_ref(1)))),
        (
            destroying(),
            snapshot(destroy_requested_to_proto_state(&credential_ref(1), &destroy_reason())),
        ),
        (
            destroyed_state(),
            snapshot(destroyed_to_proto_state(&credential_ref(1))),
        ),
        (
            cleanup_failed(),
            snapshot(cleanup_failed_to_proto_state(&credential_ref(1), &failure())),
        ),
    ];

    for (state, expected) in cases {
        assert_eq!(state, expected);
    }
}

#[test]
fn a_rotation_keeps_every_version_it_replaced() {
    let state = state_after([
        write_requested(),
        activated(),
        rotation_requested(),
        rotated(),
        event(rotation_requested_to_proto(&credential_ref(2))),
        event(rotated_to_proto(
            &credential_ref(2),
            &active_metadata(credential_ref(3)),
        )),
    ]);

    assert_eq!(
        state,
        snapshot(active_state_to_proto(
            &credential_ref(3),
            &fingerprint(),
            &[version(1), version(2)]
        ))
    );
}

#[test]
fn a_failed_rotation_returns_to_the_active_version() {
    assert_eq!(
        state_after([write_requested(), activated(), rotation_requested(), rotation_failed()]),
        active()
    );
}

#[test]
fn a_credential_can_be_revoked_while_a_rotation_is_pending() {
    assert_eq!(
        state_after([write_requested(), activated(), rotation_requested(), revoked()]),
        revoked_state()
    );
}

#[test]
fn a_failed_destroy_can_be_requested_again() {
    assert_eq!(
        state_after([
            write_requested(),
            activated(),
            revoked(),
            destroy_requested(),
            destroy_failed(),
            destroy_requested(),
        ]),
        destroying()
    );
}

#[test]
fn a_write_can_be_requested_again_after_it_failed_or_was_destroyed() {
    let after_failure = state_after([write_requested(), write_failed(), write_requested()]);
    let after_destroy = state_after([
        write_requested(),
        activated(),
        revoked(),
        destroy_requested(),
        destroyed(),
        write_requested(),
    ]);

    assert_eq!(after_failure, pending());
    assert_eq!(after_destroy, pending());
}

#[test]
fn a_repeat_write_request_must_carry_the_identity_the_stream_recorded() {
    let another_slot = event(write_requested_to_proto(&identity_of_another_slot()));

    for state in [state_after([write_requested(), write_failed()]), destroyed_state()] {
        rejected_by(
            state,
            another_slot.clone(),
            CredentialEvolveError::IdentityMismatch { key: stream() },
        );
    }
}

#[test]
fn a_write_request_is_rejected_once_a_write_started() {
    for state in [
        pending(),
        active(),
        rotating(),
        revoked_state(),
        destroying(),
        cleanup_failed(),
    ] {
        rejected_by(
            state,
            write_requested(),
            CredentialEvolveError::WriteRequestedAfterStart,
        );
    }
}

#[test]
fn events_that_need_a_precondition_are_rejected_without_it() {
    let cases = [
        (
            initial_state(),
            write_failed(),
            CredentialEvolveError::WriteFailedWithoutPendingWrite,
        ),
        (
            initial_state(),
            activated(),
            CredentialEvolveError::ActivatedWithoutPendingWrite,
        ),
        (
            initial_state(),
            rotation_requested(),
            CredentialEvolveError::RotationRequestedWithoutActiveCredential,
        ),
        (
            active(),
            rotation_failed(),
            CredentialEvolveError::RotationFailedWithoutPendingRotation,
        ),
        (
            active(),
            rotated(),
            CredentialEvolveError::RotatedWithoutPendingRotation,
        ),
        (
            initial_state(),
            revoked(),
            CredentialEvolveError::RevokedWithoutActiveCredential,
        ),
        (
            active(),
            destroy_requested(),
            CredentialEvolveError::DestroyRequestedWithoutRevokedOrCleanupFailedCredential,
        ),
        (
            active(),
            destroyed(),
            CredentialEvolveError::DestroyedWithoutDestroyRequestedCredential,
        ),
        (
            active(),
            destroy_failed(),
            CredentialEvolveError::DestroyFailedWithoutDestroyRequestedCredential,
        ),
    ];

    for (state, event, expected) in cases {
        rejected_by(state, event, expected);
    }
}

#[test]
fn an_event_naming_another_credential_than_the_one_recorded_is_rejected() {
    let other = CredentialRef::new(other_identity(), version(1));
    let cases = [
        (pending(), event(write_failed_to_proto(&other.key(), &failure()))),
        (pending(), event(activated_to_proto(&active_metadata(other.clone())))),
        (active(), event(rotation_requested_to_proto(&other))),
        (rotating(), event(rotation_failed_to_proto(&other, &failure()))),
        (
            rotating(),
            event(rotated_to_proto(&other, &active_metadata(other.next_version()))),
        ),
        (active(), event(revoked_to_proto(&other))),
        (
            revoked_state(),
            event(destroy_requested_to_proto(&other, &destroy_reason())),
        ),
        (destroying(), event(destroyed_to_proto(&other))),
        (destroying(), event(destroy_failed_to_proto(&other, &failure()))),
    ];

    for (state, event) in cases {
        assert_eq!(
            evolve(state, &event),
            Err(CredentialEvolveError::CredentialKeyMismatch {
                expected: stream(),
                actual: other.key(),
            })
        );
    }
}

#[test]
fn an_event_naming_another_version_than_the_one_recorded_is_rejected() {
    let stale = credential_ref(2);
    let cases = [
        (active(), event(rotation_requested_to_proto(&stale))),
        (rotating(), event(rotation_failed_to_proto(&stale, &failure()))),
        (
            rotating(),
            event(rotated_to_proto(&stale, &active_metadata(credential_ref(3)))),
        ),
        (active(), event(revoked_to_proto(&stale))),
        (
            revoked_state(),
            event(destroy_requested_to_proto(&stale, &destroy_reason())),
        ),
        (destroying(), event(destroyed_to_proto(&stale))),
        (destroying(), event(destroy_failed_to_proto(&stale, &failure()))),
    ];

    for (state, event) in cases {
        rejected_by(
            state,
            event,
            CredentialEvolveError::VersionMismatch {
                expected: version(1),
                actual: version(2),
            },
        );
    }
}

#[test]
fn rotation_to_a_version_that_is_not_newer_is_rejected() {
    rejected_by(
        rotating(),
        event(rotated_to_proto(
            &credential_ref(1),
            &active_metadata(credential_ref(1)),
        )),
        CredentialEvolveError::RotationVersionNotNewer,
    );
}

#[test]
fn an_event_read_from_another_stream_is_rejected_before_it_is_folded() {
    let other_stream = other_identity().key();

    assert_eq!(
        evolve_stream(&other_stream, initial_state(), &write_requested()),
        Err(CredentialEvolveError::EventFromAnotherStream {
            stream: other_stream,
            event: stream(),
        })
    );
}

#[test]
fn an_envelope_without_an_event_is_rejected() {
    let empty = v1::CredentialEvent { event: None };

    assert_eq!(
        evolve_stream(&stream(), initial_state(), &empty),
        Err(CredentialEvolveError::MissingEvent)
    );
    assert_eq!(
        evolve(initial_state(), &empty),
        Err(CredentialEvolveError::MissingEvent)
    );
}

#[test]
fn a_snapshot_without_a_state_is_rejected() {
    let empty = state_v1::CredentialStateSnapshot { state: None };

    assert_eq!(
        evolve_stream(&stream(), empty, &write_requested()),
        Err(CredentialEvolveError::MissingState)
    );
}

#[test]
fn an_event_whose_key_does_not_decode_is_rejected_before_the_stream_check() {
    let activation_without_key = event(v1::CredentialActivated {
        key: MessageField::none(),
        ..Default::default()
    });

    assert_eq!(
        evolve_stream(&stream(), pending(), &activation_without_key),
        Err(CredentialEvolveError::InvalidProto(
            CredentialProtoDecodeError::MissingField { field: "key".into() }
        ))
    );
}

#[test]
fn a_recorded_state_that_does_not_decode_is_rejected() {
    let cases = [
        (
            snapshot(state_v1::FailedCredentialWriteState::default()),
            write_requested(),
            "write_failed.identity",
        ),
        (
            snapshot(state_v1::DestroyedCredentialState::default()),
            write_requested(),
            "destroyed.identity",
        ),
        (
            snapshot(state_v1::PendingCredentialWriteState::default()),
            activated(),
            "pending_write.identity",
        ),
        (
            snapshot(state_v1::RotationPendingCredentialState::default()),
            rotation_failed(),
            "rotation_pending.active",
        ),
        (
            snapshot(state_v1::RotationPendingCredentialState::default()),
            rotated(),
            "rotation_pending.active",
        ),
        (
            snapshot(state_v1::RotationPendingCredentialState::default()),
            revoked(),
            "rotation_pending.active",
        ),
    ];

    for (state, event, field) in cases {
        assert_eq!(
            evolve(state, &event),
            Err(CredentialEvolveError::InvalidProto(
                CredentialProtoDecodeError::MissingField { field: field.into() }
            ))
        );
    }
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
    let other = other_identity().key();
    let cases = [
        (
            CredentialEvolveError::IdentityMismatch { key: key.clone() }.to_string(),
            format!("credential write request for '{key}' carries another identity than the one recorded"),
        ),
        (
            CredentialEvolveError::CredentialKeyMismatch {
                expected: key.clone(),
                actual: other.clone(),
            }
            .to_string(),
            format!("event names credential '{other}', but the state recorded '{key}'"),
        ),
        (
            CredentialEvolveError::VersionMismatch {
                expected: version(1),
                actual: version(2),
            }
            .to_string(),
            "event names credential version 2, but the state recorded 1".to_string(),
        ),
        (
            CredentialEvolveError::EventFromAnotherStream {
                stream: key.clone(),
                event: other.clone(),
            }
            .to_string(),
            format!("event names credential '{other}', but it was read from the stream of '{key}'"),
        ),
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
