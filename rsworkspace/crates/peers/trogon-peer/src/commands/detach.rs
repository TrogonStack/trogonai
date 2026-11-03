use super::domain::{AttachmentRef, PeerId, TrustedPeerOwner};
use super::state::{self, EvolveError};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DetachError {
    #[error("peer detach is not implemented")]
    NotImplemented,
    #[error("peer identity is not enrolled")]
    MissingEnrollment,
    #[error("authenticated owner does not own this peer")]
    OwnerMismatch,
    #[error("connector attachment has been fenced")]
    Fenced,
    #[error("connector already detached")]
    AlreadyDetached,
    #[error("service time moved backwards")]
    ClockRegression,
    #[error("detach timestamp is outside the supported range")]
    TimestampRange,
    #[error("invalid peer state: {0}")]
    InvalidState(#[from] EvolveError),
}

#[derive(Debug, Clone)]
pub struct Detach {
    owner: TrustedPeerOwner,
    attachment: AttachmentRef,
    now: DateTime<Utc>,
}
impl Detach {
    pub fn new(owner: TrustedPeerOwner, attachment: AttachmentRef, now: DateTime<Utc>) -> Self {
        Self { owner, attachment, now }
    }
    pub fn owner(&self) -> &TrustedPeerOwner {
        &self.owner
    }
    pub fn attachment(&self) -> &AttachmentRef {
        &self.attachment
    }
    pub fn now(&self) -> DateTime<Utc> {
        self.now
    }
}
impl Decider for Detach {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = DetachError;
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
    fn decide(_state: &PeerState, _command: &Self) -> Result<Decision<Self>, DetachError> {
        Err(DetachError::NotImplemented)
    }
}
