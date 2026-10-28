use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::super::proto::{decode_destroy_requested_state, destroy_failed_to_proto};
use super::domain::{CredentialFailureReason, CredentialRef};
use super::state::{CredentialDecideError, CredentialEvolveError, validate_same_ref};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordCredentialDestroyFailure {
    credential_ref: CredentialRef,
    reason: CredentialFailureReason,
}

impl RecordCredentialDestroyFailure {
    pub fn new(credential_ref: CredentialRef, reason: CredentialFailureReason) -> Self {
        Self { credential_ref, reason }
    }
}

impl Decider for RecordCredentialDestroyFailure {
    type StreamId = str;
    type State = state_v1::CredentialStateSnapshot;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_ref.id().as_str()
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;
        let CredentialStateSnapshotCase::DestroyRequested(pending) = current else {
            return Err(CredentialDecideError::CredentialDestroyNotPending {
                credential_id: command.credential_ref.id().clone(),
            });
        };
        let (current_ref, _) = decode_destroy_requested_state(pending)?;
        validate_same_ref(&current_ref, &command.credential_ref)?;
        Ok(Decision::event(v1::CredentialEvent {
            event: Some(destroy_failed_to_proto(&command.credential_ref, &command.reason).into()),
        }))
    }
}

impl CommandSnapshotPolicy for RecordCredentialDestroyFailure {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
