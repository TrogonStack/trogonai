use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, v1};

use super::domain::{CredentialFailureReason, CredentialRef};
use super::proto::{active_credential_ref, decode_message_field, rotation_failed_to_proto};
use super::state::{CredentialDecideError, validate_same_ref};

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
    type State = super::state::CredentialState;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_ref.id().as_str()
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
