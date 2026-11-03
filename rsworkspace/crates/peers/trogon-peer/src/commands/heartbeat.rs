use super::domain::{AttachmentRef, PeerId, PeerObservation, TrustedPeerOwner};
use super::state::{self, EvolveError};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeartbeatError {
    #[error("peer heartbeat is not implemented")]
    NotImplemented,
    #[error("peer identity is not enrolled")]
    MissingEnrollment,
    #[error("authenticated owner does not own this peer")]
    OwnerMismatch,
    #[error("connector attachment has been fenced")]
    Fenced,
    #[error("connector attachment is inactive")]
    Inactive,
    #[error("connector attachment lease expired")]
    Expired,
    #[error("service time moved backwards")]
    ClockRegression,
    #[error("reported activity moved backwards or is in the future")]
    InvalidActivity,
    #[error("attachment deadline is outside the supported range")]
    LeaseDeadlineOverflow,
    #[error("invalid peer state: {0}")]
    InvalidState(#[from] EvolveError),
}

#[derive(Debug, Clone)]
pub struct Heartbeat {
    owner: TrustedPeerOwner,
    attachment: AttachmentRef,
    observation: PeerObservation,
    now: DateTime<Utc>,
}
impl Heartbeat {
    pub fn new(
        owner: TrustedPeerOwner,
        attachment: AttachmentRef,
        observation: PeerObservation,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            owner,
            attachment,
            observation,
            now,
        }
    }
    pub fn owner(&self) -> &TrustedPeerOwner {
        &self.owner
    }
    pub fn attachment(&self) -> &AttachmentRef {
        &self.attachment
    }
    pub fn observation(&self) -> &PeerObservation {
        &self.observation
    }
    pub fn now(&self) -> DateTime<Utc> {
        self.now
    }
}
impl Decider for Heartbeat {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = HeartbeatError;
    type EvolveError = EvolveError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;
    fn stream_id(&self) -> &PeerId {
        self.attachment.peer_id()
    }
    fn initial_state() -> PeerState {
        state::initial_state()
    }
    fn evolve(state: PeerState, event: &PeerEvent) -> Result<PeerState, EvolveError> {
        state::evolve(state, event)
    }
    fn decide(_state: &PeerState, _command: &Self) -> Result<Decision<Self>, HeartbeatError> {
        Err(HeartbeatError::NotImplemented)
    }
}
