use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::super::proto::{decode_destroy_requested_state, destroyed_to_proto};
use super::domain::CredentialRef;
use super::state::{CredentialDecideError, CredentialEvolveError, validate_same_ref};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteCredentialDestroy {
    credential_ref: CredentialRef,
}

impl CompleteCredentialDestroy {
    pub fn new(credential_ref: CredentialRef) -> Self {
        Self { credential_ref }
    }
}

impl Decider for CompleteCredentialDestroy {
    type StreamId = str;
    type State = state_v1::CredentialStateSnapshot;
    type Event = v1::CredentialEvent;
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
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;
        let CredentialStateSnapshotCase::DestroyRequested(pending) = current else {
            return Err(CredentialDecideError::CredentialDestroyNotPending {
                credential_id: command.credential_ref.id().clone(),
            });
        };
        let (current_ref, _) = decode_destroy_requested_state(pending)?;
        validate_same_ref(&current_ref, &command.credential_ref)?;
        Ok(Decision::event(v1::CredentialEvent {
            event: Some(destroyed_to_proto(&command.credential_ref).into()),
        }))
    }
}

impl CommandSnapshotPolicy for CompleteCredentialDestroy {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
