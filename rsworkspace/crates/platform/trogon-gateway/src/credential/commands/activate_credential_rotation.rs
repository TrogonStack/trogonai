use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, v1};

use super::domain::CredentialMetadata;
use super::proto::{active_credential_ref, decode_message_field, rotated_to_proto};
use super::state::{
    CredentialDecideError, validate_activation_metadata, validate_newer_version, validate_same_logical_ref,
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
    type State = super::state::CredentialState;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        self.metadata.reference().id().as_str()
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;
        let CredentialStateSnapshotCase::RotationPending(rotation) = current else {
            return Err(CredentialDecideError::CredentialRotationNotPending {
                credential_id: command.metadata.reference().id().clone(),
            });
        };
        let active = decode_message_field("rotation_pending.active", &rotation.active)?;
        let previous_credential_ref = active_credential_ref(active)?;

        validate_activation_metadata(&command.metadata)?;
        validate_same_logical_ref(&previous_credential_ref, command.metadata.reference())?;
        validate_newer_version(&previous_credential_ref, command.metadata.reference())?;

        Ok(Decision::event(v1::CredentialEvent {
            event: Some(rotated_to_proto(&previous_credential_ref, &command.metadata).into()),
        }))
    }
}

impl CommandSnapshotPolicy for ActivateCredentialRotation {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
