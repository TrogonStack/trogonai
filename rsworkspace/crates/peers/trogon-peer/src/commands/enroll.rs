use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};

use super::domain::{CollaborationRef, ConnectorId, EnrollmentIdentity, PeerDisplayName, PeerId};
use super::state::{self, EvolveError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("peer enrollment is not implemented")]
pub struct EnrollError;

#[derive(Debug, Clone)]
pub struct Enroll {
    identity: EnrollmentIdentity,
    connector_id: ConnectorId,
    collaboration: CollaborationRef,
    display_name: PeerDisplayName,
    now: DateTime<Utc>,
}

impl Enroll {
    pub fn new(
        identity: EnrollmentIdentity,
        connector_id: ConnectorId,
        collaboration: CollaborationRef,
        display_name: PeerDisplayName,
        now: DateTime<Utc>,
    ) -> Self {
        Self { identity, connector_id, collaboration, display_name, now }
    }

    pub fn identity(&self) -> &EnrollmentIdentity {
        &self.identity
    }

    pub fn connector_id(&self) -> &ConnectorId {
        &self.connector_id
    }

    pub fn collaboration(&self) -> &CollaborationRef {
        &self.collaboration
    }

    pub fn display_name(&self) -> &PeerDisplayName {
        &self.display_name
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.now
    }
}

impl Decider for Enroll {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = EnrollError;
    type EvolveError = EvolveError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        self.identity.peer_id()
    }

    fn initial_state() -> Self::State {
        state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        state::evolve(state, event)
    }

    fn decide(_state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Err(EnrollError)
    }
}
