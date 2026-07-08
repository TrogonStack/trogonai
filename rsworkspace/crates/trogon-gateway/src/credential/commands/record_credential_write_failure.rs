use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::super::proto::write_failed_to_proto;
use super::domain::{CredentialFailureReason, CredentialId};
use super::state::{CredentialDecideError, CredentialEvolveError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordCredentialWriteFailure {
    credential_id: CredentialId,
    reason: CredentialFailureReason,
}

impl RecordCredentialWriteFailure {
    pub fn new(credential_id: CredentialId, reason: CredentialFailureReason) -> Self {
        Self { credential_id, reason }
    }
}

impl Decider for RecordCredentialWriteFailure {
    type StreamId = str;
    type State = state_v1::CredentialStateSnapshot;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_id.as_str()
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
            CredentialStateSnapshotCase::PendingWrite(_) => Ok(Decision::event(v1::CredentialEvent {
                event: Some(write_failed_to_proto(&command.credential_id, &command.reason).into()),
            })),
            _ => Err(CredentialDecideError::CredentialWriteNotPending {
                credential_id: command.credential_id.clone(),
            }),
        }
    }
}

impl CommandSnapshotPolicy for RecordCredentialWriteFailure {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
