use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::domain::{CredentialEvent, CredentialMetadata};
use super::state::{
    CredentialDecideError, CredentialEvolveError, CredentialState, validate_activation_metadata,
    validate_ref_matches_pending,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivateCredentialWrite {
    metadata: CredentialMetadata,
}

impl ActivateCredentialWrite {
    pub fn new(metadata: CredentialMetadata) -> Self {
        Self { metadata }
    }
}

impl Decider for ActivateCredentialWrite {
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
        let pending = match state {
            CredentialState::PendingWrite(pending) => pending,
            _ => {
                return Err(CredentialDecideError::CredentialWriteNotPending {
                    credential_id: command.metadata.reference().id().clone(),
                });
            }
        };
        validate_activation_metadata(&command.metadata)?;
        validate_ref_matches_pending(command.metadata.reference(), pending)?;

        Ok(Decision::event(CredentialEvent::Activated {
            metadata: command.metadata.clone(),
        }))
    }
}

impl CommandSnapshotPolicy for ActivateCredentialWrite {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
