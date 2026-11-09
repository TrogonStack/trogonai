use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::domain::CredentialRef;
use super::proto::{active_credential_ref, decode_message_field, revoked_to_proto};
use super::state::{CredentialDecideError, CredentialEvolveError, validate_same_ref};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevokeCredential {
    credential_ref: CredentialRef,
}

impl RevokeCredential {
    pub fn new(credential_ref: CredentialRef) -> Self {
        Self { credential_ref }
    }

    pub(crate) fn ensure_revocable(
        &self,
        state: &state_v1::CredentialStateSnapshot,
    ) -> Result<(), CredentialDecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;
        let current_ref = match current {
            CredentialStateSnapshotCase::Active(active) => active_credential_ref(active)?,
            CredentialStateSnapshotCase::RotationPending(rotation) => {
                active_credential_ref(decode_message_field("rotation_pending.active", &rotation.active)?)?
            }
            _ => {
                return Err(CredentialDecideError::CredentialNotActive {
                    credential_id: self.credential_ref.id().clone(),
                });
            }
        };
        validate_same_ref(&current_ref, &self.credential_ref)
    }
}

impl Decider for RevokeCredential {
    type StreamId = str;
    type State = state_v1::CredentialStateSnapshot;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

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
        command.ensure_revocable(state)?;
        Ok(Decision::event(v1::CredentialEvent {
            event: Some(revoked_to_proto(&command.credential_ref).into()),
        }))
    }
}

impl CommandSnapshotPolicy for RevokeCredential {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
