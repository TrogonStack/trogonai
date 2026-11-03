use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};

use super::domain::{CollaborationRef, ConnectorId, EnrollmentIdentity, PeerId};
use super::state::{self, EvolveError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachError {
    #[error("peer attachment replacement is not implemented")]
    NotImplemented,
    #[error("peer communication identity is not enrolled")]
    MissingEnrollment,
    #[error("peer enrollment identity does not match")]
    IdentityMismatch,
    #[error("collaboration reference cannot change for an enrolled identity")]
    CollaborationMismatch,
    #[error("the same connector is already attached")]
    AlreadyAttached,
    #[error("another connector holds the active attachment")]
    InUse,
    #[error("service time precedes the last attachment observation")]
    ClockRegression,
    #[error("attachment generation cannot advance")]
    GenerationOverflow,
    #[error("attachment deadline is outside the supported timestamp range")]
    LeaseDeadlineOverflow,
    #[error("peer attachment state is invalid: {0}")]
    InvalidState(#[from] EvolveError),
}

#[derive(Debug, Clone)]
pub struct Attach {
    identity: EnrollmentIdentity,
    connector_id: ConnectorId,
    collaboration: CollaborationRef,
    now: DateTime<Utc>,
}

impl Attach {
    pub fn new(
        identity: EnrollmentIdentity,
        connector_id: ConnectorId,
        collaboration: CollaborationRef,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            identity,
            connector_id,
            collaboration,
            now,
        }
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
    pub fn now(&self) -> DateTime<Utc> {
        self.now
    }
}

impl Decider for Attach {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = AttachError;
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
        Err(AttachError::NotImplemented)
    }
}
