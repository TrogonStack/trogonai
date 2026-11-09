use buffa::MessageField;

use super::*;
use crate::credential::commands::domain::{
    CredentialDestroyReason, CredentialFailureReason, CredentialFingerprint, CredentialOwnerId, CredentialVersion,
    SourceKind, StorageBackend,
};
use crate::credential::commands::proto::{
    activated_to_proto, destroy_failed_to_proto, destroy_requested_to_proto, destroyed_to_proto, revoked_to_proto,
    rotated_to_proto, rotation_failed_to_proto, rotation_requested_to_proto, write_failed_to_proto,
    write_requested_to_proto,
};

fn credential_id() -> CredentialId {
    CredentialId::new("openbao:tenant-1:webhook_secret").unwrap()
}

fn scope() -> CredentialScope {
    CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::GitHub)
}

fn credential_ref(version: u64) -> CredentialRef {
    CredentialRef::new(
        credential_id(),
        CredentialVersion::new(version).unwrap(),
        &scope(),
        CredentialKind::WebhookSecret,
    )
}

fn metadata_with(status: CredentialStatus, version: u64) -> CredentialMetadata {
    CredentialMetadata::new(
        credential_ref(version),
        status,
        StorageBackend::OpenBao,
        CredentialFingerprint::new("fingerprint").unwrap(),
    )
}

fn event(event: impl Into<CredentialEventCase>) -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(event.into()),
    }
}

fn write_requested() -> v1::CredentialEvent {
    event(write_requested_to_proto(
        &credential_id(),
        &scope(),
        CredentialKind::WebhookSecret,
    ))
}

fn activated() -> v1::CredentialEvent {
    event(activated_to_proto(&metadata_with(CredentialStatus::Active, 1)))
}

fn rotation_requested() -> v1::CredentialEvent {
    event(rotation_requested_to_proto(&credential_ref(1)))
}

fn revoked() -> v1::CredentialEvent {
    event(revoked_to_proto(&credential_ref(1)))
}

fn destroy_requested() -> v1::CredentialEvent {
    event(destroy_requested_to_proto(
        &credential_ref(1),
        &CredentialDestroyReason::new("cleanup").unwrap(),
    ))
}

fn state_after(events: impl IntoIterator<Item = v1::CredentialEvent>) -> state_v1::CredentialStateSnapshot {
    events
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap()
}

fn rejected_by(state: state_v1::CredentialStateSnapshot, event: v1::CredentialEvent, expected: CredentialEvolveError) {
    assert_eq!(evolve(state, &event), Err(expected));
}

fn other_id() -> CredentialId {
    CredentialId::new("openbao:tenant-1:other_secret").unwrap()
}

fn other_ref() -> CredentialRef {
    CredentialRef::new(
        other_id(),
        CredentialVersion::new(1).unwrap(),
        &scope(),
        CredentialKind::WebhookSecret,
    )
}

#[test]
fn write_requested_is_rejected_once_the_credential_is_active() {
    rejected_by(
        state_after([write_requested(), activated()]),
        write_requested(),
        CredentialEvolveError::WriteRequestedAfterStart,
    );
}

#[test]
fn write_failure_for_another_credential_is_rejected_while_a_write_is_pending() {
    let failed = event(write_failed_to_proto(
        &other_id(),
        &CredentialFailureReason::new("openbao write failed").unwrap(),
    ));

    rejected_by(
        state_after([write_requested()]),
        failed,
        CredentialEvolveError::CredentialRefMismatch {
            expected: credential_id(),
            actual: other_id(),
        },
    );
}

#[test]
fn events_that_need_a_precondition_are_rejected_without_it() {
    let reason = CredentialFailureReason::new("openbao failed").unwrap();
    let cases = [
        (
            initial_state(),
            event(write_failed_to_proto(&credential_id(), &reason)),
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
            state_after([write_requested(), activated()]),
            event(rotation_failed_to_proto(&credential_ref(1), &reason)),
            CredentialEvolveError::RotationFailedWithoutPendingRotation,
        ),
        (
            initial_state(),
            event(rotated_to_proto(
                &credential_ref(1),
                &metadata_with(CredentialStatus::Active, 2),
            )),
            CredentialEvolveError::RotatedWithoutPendingRotation,
        ),
        (
            initial_state(),
            revoked(),
            CredentialEvolveError::RevokedWithoutActiveCredential,
        ),
        (
            state_after([write_requested(), activated()]),
            destroy_requested(),
            CredentialEvolveError::DestroyRequestedWithoutRevokedOrCleanupFailedCredential,
        ),
        (
            initial_state(),
            event(destroyed_to_proto(&credential_ref(1))),
            CredentialEvolveError::DestroyedWithoutDestroyRequestedCredential,
        ),
        (
            initial_state(),
            event(destroy_failed_to_proto(&credential_ref(1), &reason)),
            CredentialEvolveError::DestroyFailedWithoutDestroyRequestedCredential,
        ),
    ];

    for (state, event, expected) in cases {
        rejected_by(state, event, expected);
    }
}

#[test]
fn activation_of_a_non_active_credential_is_rejected_by_decide_and_evolve() {
    let metadata = metadata_with(CredentialStatus::Pending, 1);

    assert_eq!(
        validate_activation_metadata(&metadata),
        Err(CredentialDecideError::MetadataNotActive {
            status: CredentialStatus::Pending
        })
    );
    rejected_by(
        state_after([write_requested()]),
        event(activated_to_proto(&metadata)),
        CredentialEvolveError::MetadataNotActive {
            status: CredentialStatus::Pending,
        },
    );
}

#[test]
fn activation_for_another_credential_id_is_rejected_by_decide_and_evolve() {
    let other = CredentialMetadata::new(
        other_ref(),
        CredentialStatus::Active,
        StorageBackend::OpenBao,
        CredentialFingerprint::new("fingerprint").unwrap(),
    );

    assert_eq!(
        validate_ref_matches_pending(
            other.reference(),
            &credential_id(),
            &scope(),
            CredentialKind::WebhookSecret
        ),
        Err(CredentialDecideError::CredentialRefMismatch {
            expected: credential_id(),
            actual: other_id(),
        })
    );
    rejected_by(
        state_after([write_requested()]),
        event(activated_to_proto(&other)),
        CredentialEvolveError::CredentialRefMismatch {
            expected: credential_id(),
            actual: other_id(),
        },
    );
}

#[test]
fn rotation_request_for_a_different_ref_is_rejected_by_evolve() {
    rejected_by(
        state_after([write_requested(), activated()]),
        event(rotation_requested_to_proto(&other_ref())),
        CredentialEvolveError::CredentialRefMismatch {
            expected: credential_id(),
            actual: other_id(),
        },
    );
}

#[test]
fn rotation_to_a_version_that_is_not_newer_is_rejected_by_evolve() {
    rejected_by(
        state_after([write_requested(), activated(), rotation_requested()]),
        event(rotated_to_proto(
            &credential_ref(1),
            &metadata_with(CredentialStatus::Active, 1),
        )),
        CredentialEvolveError::RotationVersionNotNewer,
    );
}

#[test]
fn missing_message_fields_surface_as_proto_errors() {
    let activation_without_metadata = v1::CredentialEvent {
        event: Some(
            v1::CredentialActivated {
                metadata: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        evolve(state_after([write_requested()]), &activation_without_metadata),
        Err(CredentialEvolveError::InvalidProto(
            CredentialProtoDecodeError::MissingField {
                field: "metadata".into()
            }
        ))
    );
}
