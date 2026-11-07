use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::domain::{CredentialMetadata, CredentialStreamId};
use super::proto::{activated_to_proto, decode_pending_write_state};
use super::state::{
    CredentialDecideError, CredentialEvolveError, validate_activation_metadata, validate_ref_matches_pending,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivateCredentialWrite {
    stream_id: CredentialStreamId,
    metadata: CredentialMetadata,
}

impl ActivateCredentialWrite {
    pub fn new(metadata: CredentialMetadata) -> Self {
        Self {
            stream_id: CredentialStreamId::from(metadata.reference()),
            metadata,
        }
    }
}

impl Decider for ActivateCredentialWrite {
    type StreamId = CredentialStreamId;
    type State = state_v1::CredentialStateSnapshot;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        &self.stream_id
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;
        let pending = match current {
            CredentialStateSnapshotCase::PendingWrite(pending) => pending,
            _ => {
                return Err(CredentialDecideError::CredentialWriteNotPending {
                    credential_id: command.metadata.reference().id().clone(),
                });
            }
        };
        let (pending_id, pending_scope, pending_kind) = decode_pending_write_state(pending)?;

        validate_activation_metadata(&command.metadata)?;
        validate_ref_matches_pending(command.metadata.reference(), &pending_id, &pending_scope, pending_kind)?;

        Ok(Decision::event(v1::CredentialEvent {
            event: Some(activated_to_proto(&command.metadata).into()),
        }))
    }
}

impl CommandSnapshotPolicy for ActivateCredentialWrite {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
