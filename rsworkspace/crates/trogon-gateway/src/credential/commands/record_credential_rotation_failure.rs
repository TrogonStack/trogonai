use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::domain::{CredentialEvent, CredentialFailureReason, CredentialRef};
use super::state::{CredentialDecideError, CredentialEvolveError, CredentialState, validate_same_ref};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordCredentialRotationFailure {
    credential_ref: CredentialRef,
    reason: CredentialFailureReason,
}

impl RecordCredentialRotationFailure {
    pub fn new(credential_ref: CredentialRef, reason: CredentialFailureReason) -> Self {
        Self { credential_ref, reason }
    }
}

impl Decider for RecordCredentialRotationFailure {
    type StreamId = str;
    type State = CredentialState;
    type Event = CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

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
        let CredentialState::RotationPending(rotation) = state else {
            return Err(CredentialDecideError::CredentialRotationNotPending {
                credential_id: command.credential_ref.id().clone(),
            });
        };
        validate_same_ref(rotation.active().credential_ref(), &command.credential_ref)?;
        Ok(Decision::event(CredentialEvent::RotationFailed {
            credential_ref: command.credential_ref.clone(),
            reason: command.reason.clone(),
        }))
    }
}

impl CommandSnapshotPolicy for RecordCredentialRotationFailure {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
