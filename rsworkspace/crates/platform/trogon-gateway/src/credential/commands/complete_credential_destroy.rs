use trogon_credential_aggregate::proto::{decode_destroy_requested_state, destroyed_to_proto};
use trogon_credential_domain::{CredentialKey, CredentialRef};
use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, v1};

use super::state::{CredentialDecideError, ensure_recorded_ref};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteCredentialDestroy {
    stream_id: CredentialKey,
    credential_ref: CredentialRef,
}

impl CompleteCredentialDestroy {
    pub fn new(credential_ref: CredentialRef) -> Self {
        Self {
            stream_id: credential_ref.key(),
            credential_ref,
        }
    }
}

impl Decider for CompleteCredentialDestroy {
    type StreamId = CredentialKey;
    type State = super::state::CredentialState;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        &self.stream_id
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;
        let CredentialStateSnapshotCase::DestroyRequested(pending) = current else {
            return Err(CredentialDecideError::CredentialDestroyNotPending {
                credential_id: command.credential_ref.id().clone(),
            });
        };
        let (current_ref, _) = decode_destroy_requested_state(pending)?;
        ensure_recorded_ref(&current_ref, &command.credential_ref)?;
        Ok(Decision::event(v1::CredentialEvent {
            event: Some(destroyed_to_proto(&command.credential_ref).into()),
        }))
    }
}

impl CommandSnapshotPolicy for CompleteCredentialDestroy {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
