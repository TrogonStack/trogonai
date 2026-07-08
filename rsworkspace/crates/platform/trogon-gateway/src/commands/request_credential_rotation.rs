use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::domain::{CredentialLifecycleEvent, CredentialRef};
use super::state::{
    CredentialLifecycleDecideError, CredentialLifecycleEvolveError, CredentialLifecycleState, validate_same_ref,
};

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
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

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
        let active = match state {
            CredentialLifecycleState::Active(active) => active,
            CredentialLifecycleState::RotationPending(_) => {
                return Err(CredentialLifecycleDecideError::CredentialRotationAlreadyPending {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
            _ => {
                return Err(CredentialLifecycleDecideError::CredentialNotActive {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
        };
        validate_same_ref(active.credential_ref(), &command.credential_ref)?;
        Ok(Decision::event(CredentialLifecycleEvent::RotationRequested {
            credential_ref: command.credential_ref.clone(),
        }))
    }
}

impl CommandSnapshotPolicy for RequestCredentialRotation {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY;
}
