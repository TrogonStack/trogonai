use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::domain::{CredentialIdentity, CredentialKey};
use super::proto::{decode_destroyed_state, decode_write_failed_state, write_requested_to_proto};
use super::state::{CredentialDecideError, CredentialEvolveError, ensure_recorded_identity};

/// Opens a credential's stream, or reopens it after a failed write or a destroy.
///
/// A stream records its identity once, so a reopening write must carry that same
/// identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestCredentialWrite {
    stream_id: CredentialKey,
    identity: CredentialIdentity,
}

impl RequestCredentialWrite {
    pub fn new(identity: CredentialIdentity) -> Self {
        Self {
            stream_id: identity.key(),
            identity,
        }
    }

    pub fn stream_id(&self) -> &CredentialKey {
        &self.stream_id
    }

    pub fn identity(&self) -> &CredentialIdentity {
        &self.identity
    }
}

impl Decider for RequestCredentialWrite {
    type StreamId = CredentialKey;
    type State = state_v1::CredentialStateSnapshot;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        &self.stream_id
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;

        match current {
            CredentialStateSnapshotCase::Missing(_) => {}
            CredentialStateSnapshotCase::WriteFailed(failed) => {
                ensure_recorded_identity(&decode_write_failed_state(failed)?.0, &command.identity)?;
            }
            CredentialStateSnapshotCase::Destroyed(destroyed) => {
                ensure_recorded_identity(decode_destroyed_state(destroyed)?.identity(), &command.identity)?;
            }
            CredentialStateSnapshotCase::Revoked(_) => {
                return Err(CredentialDecideError::Revoked {
                    credential_id: command.identity.id().clone(),
                });
            }
            _ => {
                return Err(CredentialDecideError::AlreadyExists {
                    credential_id: command.identity.id().clone(),
                });
            }
        }

        Ok(Decision::event(v1::CredentialEvent {
            event: Some(write_requested_to_proto(&command.identity).into()),
        }))
    }
}

impl CommandSnapshotPolicy for RequestCredentialWrite {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
