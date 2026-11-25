use trogon_credential_domain::{CredentialIdentity, CredentialKey, CredentialRef, CredentialVersion};
use trogonai_proto::gateway::credentials::{CredentialEventCase, CredentialStateSnapshotCase, state_v1, v1};

use crate::CredentialEvolveError;
use crate::proto::{
    active_credential_ref, active_state_to_proto, cleanup_failed_to_proto_state, decode_activated, decode_active_state,
    decode_cleanup_failed_state, decode_destroy_failed, decode_destroy_requested, decode_destroy_requested_state,
    decode_destroyed, decode_destroyed_state, decode_message_field, decode_pending_write_state, decode_revoked,
    decode_revoked_state, decode_rotated, decode_rotation_failed, decode_rotation_requested, decode_write_failed,
    decode_write_failed_state, decode_write_requested, destroy_requested_to_proto_state, destroyed_to_proto_state,
    event_key, pending_write_to_proto_state, revoked_to_proto_state, rotation_pending_to_proto_state,
    write_failed_to_proto_state,
};

pub fn initial_state() -> state_v1::CredentialStateSnapshot {
    state_v1::CredentialStateSnapshot {
        state: Some(state_v1::CredentialMissingState {}.into()),
    }
}

/// Folds an event read from the stream of `stream` into its state.
///
/// [`evolve`] holds every event to the identity the stream recorded. This also
/// holds the first event to the stream it was read from, which is the one check
/// a state cannot make on its own before it has recorded an identity.
pub fn evolve_stream(
    stream: &CredentialKey,
    state: state_v1::CredentialStateSnapshot,
    event: &v1::CredentialEvent,
) -> Result<state_v1::CredentialStateSnapshot, CredentialEvolveError> {
    let case = event.event.as_ref().ok_or(CredentialEvolveError::MissingEvent)?;
    let key = event_key(case)?;
    if &key != stream {
        return Err(CredentialEvolveError::EventFromAnotherStream {
            stream: stream.clone(),
            event: key,
        });
    }
    evolve(state, event)
}

pub fn evolve(
    state: state_v1::CredentialStateSnapshot,
    event: &v1::CredentialEvent,
) -> Result<state_v1::CredentialStateSnapshot, CredentialEvolveError> {
    let event = event.event.as_ref().ok_or(CredentialEvolveError::MissingEvent)?;
    let current = state.state.as_ref().ok_or(CredentialEvolveError::MissingState)?;

    let next = match event {
        CredentialEventCase::WriteRequested(inner) => {
            let recorded = match current {
                CredentialStateSnapshotCase::Missing(_) => None,
                CredentialStateSnapshotCase::WriteFailed(failed) => Some(decode_write_failed_state(failed)?.0),
                CredentialStateSnapshotCase::Destroyed(destroyed) => {
                    Some(decode_destroyed_state(destroyed)?.identity().clone())
                }
                _ => return Err(CredentialEvolveError::WriteRequestedAfterStart),
            };
            let identity = decode_write_requested(inner)?;
            if let Some(recorded) = recorded
                && recorded != identity
            {
                return Err(CredentialEvolveError::IdentityMismatch { key: recorded.key() });
            }
            pending_write_to_proto_state(&identity).into()
        }
        CredentialEventCase::WriteFailed(inner) => match current {
            CredentialStateSnapshotCase::PendingWrite(pending) => {
                let identity = decode_pending_write_state(pending)?;
                let (key, reason) = decode_write_failed(inner)?;
                ensure_event_key(&identity, &key)?;
                write_failed_to_proto_state(&identity, &reason).into()
            }
            _ => return Err(CredentialEvolveError::WriteFailedWithoutPendingWrite),
        },
        CredentialEventCase::Activated(inner) => match current {
            CredentialStateSnapshotCase::PendingWrite(pending) => {
                let identity = decode_pending_write_state(pending)?;
                let (key, version, fingerprint) = decode_activated(inner)?;
                ensure_event_key(&identity, &key)?;
                active_state_to_proto(&CredentialRef::new(identity, version), &fingerprint, &[]).into()
            }
            _ => return Err(CredentialEvolveError::ActivatedWithoutPendingWrite),
        },
        CredentialEventCase::RotationRequested(inner) => match current {
            CredentialStateSnapshotCase::Active(active) => {
                let current_ref = active_credential_ref(active)?;
                let (key, version) = decode_rotation_requested(inner)?;
                ensure_event_ref(&current_ref, &key, version)?;
                rotation_pending_to_proto_state((**active).clone()).into()
            }
            _ => return Err(CredentialEvolveError::RotationRequestedWithoutActiveCredential),
        },
        CredentialEventCase::RotationFailed(inner) => match current {
            CredentialStateSnapshotCase::RotationPending(rotation) => {
                let active = decode_message_field("rotation_pending.active", &rotation.active)?;
                let current_ref = active_credential_ref(active)?;
                let (key, version, _reason) = decode_rotation_failed(inner)?;
                ensure_event_ref(&current_ref, &key, version)?;
                active.clone().into()
            }
            _ => return Err(CredentialEvolveError::RotationFailedWithoutPendingRotation),
        },
        CredentialEventCase::Rotated(inner) => match current {
            CredentialStateSnapshotCase::RotationPending(rotation) => {
                let active = decode_message_field("rotation_pending.active", &rotation.active)?;
                let (metadata, previous) = decode_active_state("rotation_pending.active", active)?;
                let (key, previous_version, version, fingerprint) = decode_rotated(inner)?;
                ensure_event_ref(metadata.reference(), &key, previous_version)?;
                if version <= previous_version {
                    return Err(CredentialEvolveError::RotationVersionNotNewer);
                }
                let mut previous_versions: Vec<_> = previous.iter().map(CredentialRef::version).collect();
                previous_versions.push(previous_version);
                active_state_to_proto(
                    &metadata.reference().with_version(version),
                    &fingerprint,
                    &previous_versions,
                )
                .into()
            }
            _ => return Err(CredentialEvolveError::RotatedWithoutPendingRotation),
        },
        CredentialEventCase::Revoked(inner) => {
            let current_ref = match current {
                CredentialStateSnapshotCase::Active(active) => active_credential_ref(active)?,
                CredentialStateSnapshotCase::RotationPending(rotation) => {
                    active_credential_ref(decode_message_field("rotation_pending.active", &rotation.active)?)?
                }
                _ => return Err(CredentialEvolveError::RevokedWithoutActiveCredential),
            };
            let (key, version) = decode_revoked(inner)?;
            ensure_event_ref(&current_ref, &key, version)?;
            revoked_to_proto_state(&current_ref).into()
        }
        CredentialEventCase::DestroyRequested(inner) => {
            let current_ref = match current {
                CredentialStateSnapshotCase::Revoked(revoked) => decode_revoked_state(revoked)?,
                CredentialStateSnapshotCase::CleanupFailed(cleanup_failed) => {
                    decode_cleanup_failed_state(cleanup_failed)?.0
                }
                _ => return Err(CredentialEvolveError::DestroyRequestedWithoutRevokedOrCleanupFailedCredential),
            };
            let (key, version, reason) = decode_destroy_requested(inner)?;
            ensure_event_ref(&current_ref, &key, version)?;
            destroy_requested_to_proto_state(&current_ref, &reason).into()
        }
        CredentialEventCase::Destroyed(inner) => match current {
            CredentialStateSnapshotCase::DestroyRequested(pending) => {
                let (current_ref, _) = decode_destroy_requested_state(pending)?;
                let (key, version) = decode_destroyed(inner)?;
                ensure_event_ref(&current_ref, &key, version)?;
                destroyed_to_proto_state(&current_ref).into()
            }
            _ => return Err(CredentialEvolveError::DestroyedWithoutDestroyRequestedCredential),
        },
        CredentialEventCase::DestroyFailed(inner) => match current {
            CredentialStateSnapshotCase::DestroyRequested(pending) => {
                let (current_ref, _) = decode_destroy_requested_state(pending)?;
                let (key, version, reason) = decode_destroy_failed(inner)?;
                ensure_event_ref(&current_ref, &key, version)?;
                cleanup_failed_to_proto_state(&current_ref, &reason).into()
            }
            _ => return Err(CredentialEvolveError::DestroyFailedWithoutDestroyRequestedCredential),
        },
    };

    Ok(state_v1::CredentialStateSnapshot { state: Some(next) })
}

fn ensure_event_key(recorded: &CredentialIdentity, event: &CredentialKey) -> Result<(), CredentialEvolveError> {
    let expected = recorded.key();
    if &expected != event {
        return Err(CredentialEvolveError::CredentialKeyMismatch {
            expected,
            actual: event.clone(),
        });
    }
    Ok(())
}

fn ensure_event_ref(
    recorded: &CredentialRef,
    key: &CredentialKey,
    version: CredentialVersion,
) -> Result<(), CredentialEvolveError> {
    ensure_event_key(recorded.identity(), key)?;
    if recorded.version() != version {
        return Err(CredentialEvolveError::VersionMismatch {
            expected: recorded.version(),
            actual: version,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
