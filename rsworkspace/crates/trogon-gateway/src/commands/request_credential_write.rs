use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};

use super::domain::{CredentialId, CredentialKind, CredentialLifecycleEvent, CredentialOwnerId, SourceKind};
use super::state::{CredentialLifecycleDecideError, CredentialLifecycleEvolveError, CredentialLifecycleState};

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
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

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
        match state {
            CredentialLifecycleState::Missing => Ok(Decision::event(CredentialLifecycleEvent::WriteRequested {
                credential_id: command.credential_id.clone(),
                owner_id: command.owner_id.clone(),
                source: command.source,
                kind: command.kind,
            })),
            CredentialLifecycleState::Revoked(_) => Err(CredentialLifecycleDecideError::Revoked {
                credential_id: command.credential_id.clone(),
            }),
            _ => Err(CredentialLifecycleDecideError::AlreadyExists {
                credential_id: command.credential_id.clone(),
            }),
        }
    }
}

impl CommandSnapshotPolicy for RequestCredentialWrite {
    type SnapshotPolicy = FrequencySnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY;
}
