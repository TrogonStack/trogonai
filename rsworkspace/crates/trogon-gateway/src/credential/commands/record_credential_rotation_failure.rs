use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::super::proto::{active_credential_ref, decode_message_field, rotation_failed_to_proto};
use super::domain::{CredentialFailureReason, CredentialRef};
use super::state::{CredentialDecideError, CredentialEvolveError, validate_same_ref};

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
        let CredentialStateSnapshotCase::RotationPending(rotation) = current else {
            return Err(CredentialDecideError::CredentialRotationNotPending {
                credential_id: command.credential_ref.id().clone(),
            });
        };
        let active = decode_message_field("rotation_pending.active", &rotation.active)?;
        let current_ref = active_credential_ref(active)?;
        validate_same_ref(&current_ref, &command.credential_ref)?;
        Ok(Decision::event(v1::CredentialEvent {
            event: Some(rotation_failed_to_proto(&command.credential_ref, &command.reason).into()),
        }))
    }
}

impl CommandSnapshotPolicy for RecordCredentialRotationFailure {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
