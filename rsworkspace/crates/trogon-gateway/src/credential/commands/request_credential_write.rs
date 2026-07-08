use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1, v1};

use super::super::proto::write_requested_to_proto;
use super::domain::{CredentialId, CredentialKind, CredentialOwnerId, SourceKind};
use super::state::{CredentialDecideError, CredentialEvolveError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestCredentialWrite {
    credential_id: CredentialId,
    owner_id: CredentialOwnerId,
    source: SourceKind,
    kind: CredentialKind,
}

impl RequestCredentialWrite {
    pub fn new(
        credential_id: CredentialId,
        owner_id: CredentialOwnerId,
        source: SourceKind,
        kind: CredentialKind,
    ) -> Self {
        Self {
            credential_id,
            owner_id,
            source,
            kind,
        }
    }

    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        &self.owner_id
    }

    pub fn source(&self) -> SourceKind {
        self.source
    }

    pub fn kind(&self) -> CredentialKind {
        self.kind
    }
}

impl Decider for RequestCredentialWrite {
    type StreamId = str;
    type State = state_v1::CredentialStateSnapshot;
    type Event = v1::CredentialEvent;
    type DecideError = CredentialDecideError;
    type EvolveError = CredentialEvolveError;

    const WRITE_PRECONDITION: Option<WritePrecondition> = Some(WritePrecondition::NoStream);

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_id.as_str()
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let current = state.state.as_ref().ok_or(CredentialDecideError::MissingState)?;

        match current {
            CredentialStateSnapshotCase::Missing(_) => Ok(Decision::event(v1::CredentialEvent {
                event: Some(
                    write_requested_to_proto(&command.credential_id, &command.owner_id, command.source, command.kind)
                        .into(),
                ),
            })),
            CredentialStateSnapshotCase::Revoked(_) => Err(CredentialDecideError::Revoked {
                credential_id: command.credential_id.clone(),
            }),
            _ => Err(CredentialDecideError::AlreadyExists {
                credential_id: command.credential_id.clone(),
            }),
        }
    }
}

impl CommandSnapshotPolicy for RequestCredentialWrite {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
}
