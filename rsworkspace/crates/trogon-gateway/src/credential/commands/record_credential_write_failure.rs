use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::super::domain::{CredentialEvent, CredentialFailureReason, CredentialId};
use super::super::state::{CredentialDecideError, CredentialEvolveError, CredentialState};

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
    type State = CredentialState;
    type Event = CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_id.as_str()
    }

    fn initial_state() -> Self::State {
        super::super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::super::state::evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        match state {
            CredentialState::PendingWrite(_) => Ok(Decision::event(CredentialEvent::WriteFailed {
                credential_id: command.credential_id.clone(),
                reason: command.reason.clone(),
            })),
            _ => Err(CredentialDecideError::CredentialWriteNotPending {
                credential_id: command.credential_id.clone(),
            }),
        }
    }
}

impl CommandSnapshotPolicy for RecordCredentialWriteFailure {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
