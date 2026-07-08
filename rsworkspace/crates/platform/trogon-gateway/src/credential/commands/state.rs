use trogonai_proto::gateway::credentials::{CredentialEventCase, CredentialStateSnapshotCase, state_v1, v1};

use super::super::proto::{
    CredentialProtoDecodeError, active_credential_ref, active_state_to_proto, decode_active_state,
    decode_credential_metadata, decode_message_field, decode_pending_write_state, decode_revoked, decode_rotated,
    decode_rotation_failed, decode_rotation_requested, decode_write_failed, decode_write_requested,
    pending_write_to_proto_state, revoked_to_proto_state, rotation_pending_to_proto_state, write_failed_to_proto_state,
};
use super::domain::{
    CredentialId, CredentialKind, CredentialMetadata, CredentialOwnerId, CredentialRef, CredentialStatus, SourceKind,
};

pub fn initial_state() -> state_v1::CredentialStateSnapshot {
    state_v1::CredentialStateSnapshot {
        state: Some(state_v1::CredentialMissingState {}.into()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialDecideError {
    #[error("credential state snapshot carried no state")]
    MissingState,
    #[error("credential '{credential_id}' already exists")]
    AlreadyExists { credential_id: CredentialId },
    #[error("credential '{credential_id}' was revoked")]
    Revoked { credential_id: CredentialId },
    #[error("credential '{credential_id}' does not exist")]
    CredentialNotFound { credential_id: CredentialId },
    #[error("credential '{credential_id}' write is not pending")]
    CredentialWriteNotPending { credential_id: CredentialId },
    #[error("credential '{credential_id}' is not active")]
    CredentialNotActive { credential_id: CredentialId },
    #[error("credential '{credential_id}' rotation is already pending")]
    CredentialRotationAlreadyPending { credential_id: CredentialId },
    #[error("credential '{credential_id}' rotation is not pending")]
    CredentialRotationNotPending { credential_id: CredentialId },
    #[error("credential ref does not match credential stream: expected '{expected}', got '{actual}'")]
    CredentialRefMismatch {
        expected: CredentialId,
        actual: CredentialId,
    },
    #[error("credential metadata must describe an active credential: {status}")]
    MetadataNotActive { status: CredentialStatus },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
    #[error("persisted credential state is invalid: {0}")]
    InvalidProto(#[from] CredentialProtoDecodeError),
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialEvolveError {
    #[error("credential event envelope carried no event")]
    MissingEvent,
    #[error("credential state snapshot carried no state")]
    MissingState,
    #[error("credential write was requested after write already started")]
    WriteRequestedAfterStart,
    #[error("credential write failure was recorded without a pending write")]
    WriteFailedWithoutPendingWrite,
    #[error("credential was activated without a pending write")]
    ActivatedWithoutPendingWrite,
    #[error("credential rotation was requested without an active credential")]
    RotationRequestedWithoutActiveCredential,
    #[error("credential rotation failure was recorded without a pending rotation")]
    RotationFailedWithoutPendingRotation,
    #[error("credential was rotated without a pending rotation")]
    RotatedWithoutPendingRotation,
    #[error("credential was revoked without an active credential")]
    RevokedWithoutActiveCredential,
    #[error("event credential ref does not match credential stream: expected '{expected}', got '{actual}'")]
    CredentialRefMismatch {
        expected: CredentialId,
        actual: CredentialId,
    },
    #[error("event credential metadata must describe an active credential: {status}")]
    MetadataNotActive { status: CredentialStatus },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
    #[error("persisted credential state is invalid: {0}")]
    InvalidProto(#[from] CredentialProtoDecodeError),
}

pub fn evolve(
    state: state_v1::CredentialStateSnapshot,
    event: &v1::CredentialEvent,
) -> Result<state_v1::CredentialStateSnapshot, CredentialEvolveError> {
    let event = event.event.as_ref().ok_or(CredentialEvolveError::MissingEvent)?;
    let current = state.state.as_ref().ok_or(CredentialEvolveError::MissingState)?;

    let next = match event {
        CredentialEventCase::WriteRequested(inner) => match current {
            CredentialStateSnapshotCase::Missing(_) => {
                let (credential_id, owner_id, source, kind) = decode_write_requested(inner)?;
                pending_write_to_proto_state(&credential_id, &owner_id, source, kind).into()
            }
            _ => return Err(CredentialEvolveError::WriteRequestedAfterStart),
        },
        CredentialEventCase::WriteFailed(inner) => match current {
            CredentialStateSnapshotCase::PendingWrite(pending) => {
                let (pending_id, _, _, _) = decode_pending_write_state(pending)?;
                let (event_id, reason) = decode_write_failed(inner)?;
                if pending_id != event_id {
                    return Err(CredentialEvolveError::CredentialRefMismatch {
                        expected: pending_id,
                        actual: event_id,
                    });
                }
                write_failed_to_proto_state(&event_id, &reason).into()
            }
            _ => return Err(CredentialEvolveError::WriteFailedWithoutPendingWrite),
        },
        CredentialEventCase::Activated(inner) => match current {
            CredentialStateSnapshotCase::PendingWrite(pending) => {
                let (pending_id, pending_owner, pending_source, pending_kind) = decode_pending_write_state(pending)?;
                let metadata =
                    decode_credential_metadata("metadata", decode_message_field("metadata", &inner.metadata)?)?;
                validate_activation_metadata_for_evolve(&metadata)?;
                validate_ref_matches_pending_for_evolve(
                    metadata.reference(),
                    &pending_id,
                    &pending_owner,
                    pending_source,
                    pending_kind,
                )?;
                active_state_to_proto(&metadata, &[]).into()
            }
            _ => return Err(CredentialEvolveError::ActivatedWithoutPendingWrite),
        },
        CredentialEventCase::RotationRequested(inner) => match current {
            CredentialStateSnapshotCase::Active(active) => {
                let current_ref = active_credential_ref(active)?;
                let event_ref = decode_rotation_requested(inner)?;
                validate_same_ref_for_evolve(&current_ref, &event_ref)?;
                rotation_pending_to_proto_state((**active).clone()).into()
            }
            _ => return Err(CredentialEvolveError::RotationRequestedWithoutActiveCredential),
        },
        CredentialEventCase::RotationFailed(inner) => match current {
            CredentialStateSnapshotCase::RotationPending(rotation) => {
                let active = decode_message_field("rotation_pending.active", &rotation.active)?;
                let current_ref = active_credential_ref(active)?;
                let (event_ref, _reason) = decode_rotation_failed(inner)?;
                validate_same_ref_for_evolve(&current_ref, &event_ref)?;
                active.clone().into()
            }
            _ => return Err(CredentialEvolveError::RotationFailedWithoutPendingRotation),
        },
        CredentialEventCase::Rotated(inner) => match current {
            CredentialStateSnapshotCase::RotationPending(rotation) => {
                let active = decode_message_field("rotation_pending.active", &rotation.active)?;
                let current_ref = active_credential_ref(active)?;
                let (_, mut previous_versions) = decode_active_state("rotation_pending.active", active)?;
                let (previous_ref, metadata) = decode_rotated(inner)?;
                validate_same_ref_for_evolve(&current_ref, &previous_ref)?;
                validate_activation_metadata_for_evolve(&metadata)?;
                validate_same_logical_ref_for_evolve(&previous_ref, metadata.reference())?;
                validate_newer_version_for_evolve(&previous_ref, metadata.reference())?;
                previous_versions.push(previous_ref);
                active_state_to_proto(&metadata, &previous_versions).into()
            }
            _ => return Err(CredentialEvolveError::RotatedWithoutPendingRotation),
        },
        CredentialEventCase::Revoked(inner) => match current {
            CredentialStateSnapshotCase::Active(active) => {
                let current_ref = active_credential_ref(active)?;
                let event_ref = decode_revoked(inner)?;
                validate_same_ref_for_evolve(&current_ref, &event_ref)?;
                revoked_to_proto_state(&event_ref).into()
            }
            _ => return Err(CredentialEvolveError::RevokedWithoutActiveCredential),
        },
    };

    Ok(state_v1::CredentialStateSnapshot { state: Some(next) })
}

pub(crate) fn validate_activation_metadata(metadata: &CredentialMetadata) -> Result<(), CredentialDecideError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialDecideError::MetadataNotActive { status });
    }
    Ok(())
}

pub(crate) fn validate_activation_metadata_for_evolve(
    metadata: &CredentialMetadata,
) -> Result<(), CredentialEvolveError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialEvolveError::MetadataNotActive { status });
    }
    Ok(())
}

pub(crate) fn validate_ref_matches_pending(
    credential_ref: &CredentialRef,
    expected_id: &CredentialId,
    expected_owner_id: &CredentialOwnerId,
    expected_source: SourceKind,
    expected_kind: CredentialKind,
) -> Result<(), CredentialDecideError> {
    if credential_ref.id() != expected_id {
        return Err(CredentialDecideError::CredentialRefMismatch {
            expected: expected_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    if credential_ref.owner_id() != expected_owner_id
        || credential_ref.source() != expected_source
        || credential_ref.kind() != expected_kind
    {
        return Err(CredentialDecideError::CredentialRefMismatch {
            expected: expected_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_ref_matches_pending_for_evolve(
    credential_ref: &CredentialRef,
    expected_id: &CredentialId,
    expected_owner_id: &CredentialOwnerId,
    expected_source: SourceKind,
    expected_kind: CredentialKind,
) -> Result<(), CredentialEvolveError> {
    if credential_ref.id() != expected_id {
        return Err(CredentialEvolveError::CredentialRefMismatch {
            expected: expected_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    if credential_ref.owner_id() != expected_owner_id
        || credential_ref.source() != expected_source
        || credential_ref.kind() != expected_kind
    {
        return Err(CredentialEvolveError::CredentialRefMismatch {
            expected: expected_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_ref(expected: &CredentialRef, actual: &CredentialRef) -> Result<(), CredentialDecideError> {
    if expected != actual {
        return Err(CredentialDecideError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_ref_for_evolve(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialEvolveError> {
    if expected != actual {
        return Err(CredentialEvolveError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_logical_ref(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialDecideError> {
    if expected.id() != actual.id()
        || expected.owner_id() != actual.owner_id()
        || expected.source() != actual.source()
        || expected.kind() != actual.kind()
    {
        return Err(CredentialDecideError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_logical_ref_for_evolve(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialEvolveError> {
    if expected.id() != actual.id()
        || expected.owner_id() != actual.owner_id()
        || expected.source() != actual.source()
        || expected.kind() != actual.kind()
    {
        return Err(CredentialEvolveError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_newer_version(
    current: &CredentialRef,
    next: &CredentialRef,
) -> Result<(), CredentialDecideError> {
    if next.version() <= current.version() {
        return Err(CredentialDecideError::RotationVersionNotNewer);
    }
    Ok(())
}

pub(crate) fn validate_newer_version_for_evolve(
    current: &CredentialRef,
    next: &CredentialRef,
) -> Result<(), CredentialEvolveError> {
    if next.version() <= current.version() {
        return Err(CredentialEvolveError::RotationVersionNotNewer);
    }
    Ok(())
}
