use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, v1};

use super::domain::{CredentialDestroyReason, CredentialRef};
use super::proto::{
    decode_cleanup_failed_state, decode_destroyed_state, decode_revoked_state, destroy_requested_to_proto,
};
use super::state::{CredentialDecideError, validate_same_ref};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestCredentialDestroy {
    credential_ref: CredentialRef,
    reason: CredentialDestroyReason,
}

impl RequestCredentialDestroy {
    pub fn new(credential_ref: CredentialRef, reason: CredentialDestroyReason) -> Self {
        Self { credential_ref, reason }
    }

    pub fn credential_ref(&self) -> &CredentialRef {
        &self.credential_ref
    }
}

impl Decider for RequestCredentialDestroy {
    type StreamId = str;
    type State = super::state::CredentialState;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_ref.id().as_str()
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;
        match current {
            CredentialStateSnapshotCase::Revoked(revoked) => {
                let current_ref = decode_revoked_state(revoked)?;
                validate_same_ref(&current_ref, &command.credential_ref)?;
            }
            CredentialStateSnapshotCase::CleanupFailed(cleanup_failed) => {
                let (current_ref, _) = decode_cleanup_failed_state(cleanup_failed)?;
                validate_same_ref(&current_ref, &command.credential_ref)?;
            }
            CredentialStateSnapshotCase::Destroyed(destroyed) => {
                let current_ref = decode_destroyed_state(destroyed)?;
                validate_same_ref(&current_ref, &command.credential_ref)?;
                return Err(CredentialDecideError::CredentialAlreadyDestroyed {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
            CredentialStateSnapshotCase::DestroyRequested(_) => {
                return Err(CredentialDecideError::CredentialDestroyAlreadyRequested {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
            _ => {
                return Err(CredentialDecideError::CredentialNotRevokedOrExpired {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
        }

        Ok(Decision::event(v1::CredentialEvent {
            event: Some(destroy_requested_to_proto(&command.credential_ref, &command.reason).into()),
        }))
    }
}

impl CommandSnapshotPolicy for RequestCredentialDestroy {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
