use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::domain::{CredentialFailureReason, CredentialId, CredentialLifecycleEvent};
use super::state::{CredentialLifecycleDecideError, CredentialLifecycleEvolveError, CredentialLifecycleState};

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
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

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
        match state {
            CredentialLifecycleState::PendingWrite(_) => Ok(Decision::event(CredentialLifecycleEvent::WriteFailed {
                credential_id: command.credential_id.clone(),
                reason: command.reason.clone(),
            })),
            _ => Err(CredentialLifecycleDecideError::CredentialWriteNotPending {
                credential_id: command.credential_id.clone(),
            }),
        }
    }
}

impl CommandSnapshotPolicy for RecordCredentialWriteFailure {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY;
}
