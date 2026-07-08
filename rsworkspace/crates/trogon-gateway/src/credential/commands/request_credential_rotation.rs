use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::super::domain::{CredentialEvent, CredentialRef};
use super::super::state::{CredentialDecideError, CredentialEvolveError, CredentialState, validate_same_ref};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestCredentialRotation {
    credential_ref: CredentialRef,
}

impl RequestCredentialRotation {
    pub fn new(credential_ref: CredentialRef) -> Self {
        Self { credential_ref }
    }

    pub fn credential_ref(&self) -> &CredentialRef {
        &self.credential_ref
    }
}

impl Decider for RequestCredentialRotation {
    type StreamId = str;
    type State = CredentialState;
    type Event = CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_ref.id().as_str()
    }

    fn initial_state() -> Self::State {
        super::super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::super::state::evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let active = match state {
            CredentialState::Active(active) => active,
            CredentialState::RotationPending(_) => {
                return Err(CredentialDecideError::CredentialRotationAlreadyPending {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
            _ => {
                return Err(CredentialDecideError::CredentialNotActive {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
        };
        validate_same_ref(active.credential_ref(), &command.credential_ref)?;
        Ok(Decision::event(CredentialEvent::RotationRequested {
            credential_ref: command.credential_ref.clone(),
        }))
    }
}

impl CommandSnapshotPolicy for RequestCredentialRotation {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
