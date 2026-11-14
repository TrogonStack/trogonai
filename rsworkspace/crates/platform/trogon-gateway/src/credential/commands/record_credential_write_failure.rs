use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::domain::{CredentialFailureReason, CredentialKey};
use super::proto::{decode_pending_write_state, write_failed_to_proto};
use super::state::{CredentialDecideError, CredentialEvolveError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordCredentialWriteFailure {
    stream_id: CredentialKey,
    reason: CredentialFailureReason,
}

impl RecordCredentialWriteFailure {
    pub fn new(stream_id: CredentialKey, reason: CredentialFailureReason) -> Self {
        Self { stream_id, reason }
    }
}

impl Decider for RecordCredentialWriteFailure {
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

        let CredentialStateSnapshotCase::PendingWrite(pending) = current else {
            return Err(CredentialDecideError::CredentialWriteNotPending {
                credential_id: command.stream_id.credential_id().clone(),
            });
        };
        if decode_pending_write_state(pending)?.key() != command.stream_id {
            return Err(CredentialDecideError::IdentityMismatch {
                key: command.stream_id.clone(),
            });
        }
        Ok(Decision::event(v1::CredentialEvent {
            event: Some(write_failed_to_proto(&command.stream_id, &command.reason).into()),
        }))
    }
}

impl CommandSnapshotPolicy for RecordCredentialWriteFailure {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
