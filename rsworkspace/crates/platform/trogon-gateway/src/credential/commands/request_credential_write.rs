use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, v1};

use super::domain::{CredentialId, CredentialKind, CredentialScope};
use super::proto::write_requested_to_proto;
use super::state::CredentialDecideError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestCredentialWrite {
    credential_id: CredentialId,
    scope: CredentialScope,
    kind: CredentialKind,
}

impl RequestCredentialWrite {
    pub fn new(credential_id: CredentialId, scope: CredentialScope, kind: CredentialKind) -> Self {
        Self {
            credential_id,
            scope,
            kind,
        }
    }

    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }

    pub fn scope(&self) -> &CredentialScope {
        &self.scope
    }

    pub fn kind(&self) -> CredentialKind {
        self.kind
    }
}

impl Decider for RequestCredentialWrite {
    type StreamId = str;
    type State = super::state::CredentialState;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_id.as_str()
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;

        match current {
            CredentialStateSnapshotCase::Missing(_)
            | CredentialStateSnapshotCase::WriteFailed(_)
            | CredentialStateSnapshotCase::Destroyed(_) => {}
            CredentialStateSnapshotCase::Revoked(_) => {
                return Err(CredentialDecideError::Revoked {
                    credential_id: command.credential_id.clone(),
                });
            }
            _ => {
                return Err(CredentialDecideError::AlreadyExists {
                    credential_id: command.credential_id.clone(),
                });
            }
        }

        Ok(Decision::event(v1::CredentialEvent {
            event: Some(write_requested_to_proto(&command.credential_id, &command.scope, command.kind).into()),
        }))
    }
}

impl CommandSnapshotPolicy for RequestCredentialWrite {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
