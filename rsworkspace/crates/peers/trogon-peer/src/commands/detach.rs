use super::domain::{AttachmentRef, PeerId, TrustedPeerOwner};
use super::state::{self, EvolveError};
use buffa::MessageField;
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{
    state_v1alpha1::PeerState,
    v1alpha1::{self as proto, PeerEvent},
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DetachError {
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
    fn decide(state: &PeerState, command: &Self) -> Result<Decision<Self>, DetachError> {
        let enrollment = state.enrollment.as_option().ok_or(DetachError::MissingEnrollment)?;
        let identity = state::parse_enrollment(enrollment)?;
        if identity.owner() != &command.owner {
            return Err(DetachError::OwnerMismatch);
        }
        let previous = state.attachment.as_option().ok_or(EvolveError::InvalidAttachment)?;
        state::validate_attachment(enrollment, previous)?;
        if command.attachment.peer_id() != identity.peer_id()
            || previous.attachment_ref.connector_id.as_deref() != Some(command.attachment.connector_id().as_str())
            || previous.attachment_ref.generation != Some(command.attachment.generation())
        {
            return Err(DetachError::Fenced);
        }
        if previous.status.and_then(|status| status.as_known())
            == Some(proto::AttachmentStatus::ATTACHMENT_STATUS_DETACHED)
        {
            return Err(DetachError::AlreadyDetached);
        }
        let observed = state::read_time(previous.observed_at.as_option(), "attachment.observed_at")?;
        if command.now < observed {
            return Err(DetachError::ClockRegression);
        }
        if !(-62_135_596_800..=253_402_300_799).contains(&command.now.timestamp())
            || command.now.timestamp_subsec_nanos() >= 1_000_000_000
        {
            return Err(DetachError::TimestampRange);
        }
        Ok(Decision::event(PeerEvent {
            event: Some(
                proto::PeerDetached {
                    attachment_ref: previous.attachment_ref.clone(),
                    detached_at: MessageField::some(timestamp_from_datetime(&command.now)),
                }
                .into(),
            ),
        }))
    }
}
