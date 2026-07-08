use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::domain::{CredentialEvent, CredentialMetadata};
use super::state::{
    CredentialDecideError, CredentialEvolveError, CredentialState, validate_activation_metadata,
    validate_newer_version, validate_same_logical_ref,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivateCredentialRotation {
    metadata: CredentialMetadata,
}

impl ActivateCredentialRotation {
    pub fn new(metadata: CredentialMetadata) -> Self {
        Self { metadata }
    }
}

impl Decider for ActivateCredentialRotation {
    type StreamId = str;
    type State = CredentialState;
    type Event = CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.metadata.reference().id().as_str()
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
                credential_id: command.metadata.reference().id().clone(),
            });
        };
        validate_activation_metadata(&command.metadata)?;
        validate_same_logical_ref(rotation.active.credential_ref(), command.metadata.reference())?;
        validate_newer_version(rotation.active.credential_ref(), command.metadata.reference())?;

        Ok(Decision::event(CredentialEvent::Rotated {
            previous_credential_ref: rotation.active.credential_ref().clone(),
            metadata: command.metadata.clone(),
        }))
    }
}

impl CommandSnapshotPolicy for ActivateCredentialRotation {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
