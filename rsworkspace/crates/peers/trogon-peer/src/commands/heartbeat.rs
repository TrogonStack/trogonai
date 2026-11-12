use super::constants::ATTACHMENT_LEASE;
use super::domain::{AttachmentRef, PeerId, PeerObservation, TrustedPeerOwner};
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
pub enum HeartbeatError {
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
    fn decide(state: &PeerState, command: &Self) -> Result<Decision<Self>, HeartbeatError> {
        let previous = guard_live_attachment(state, &command.owner, &command.attachment, command.now)?;
        let previous_activity = state
            .active_at
            .as_option()
            .map(|at| state::read_time(Some(at), "active_at"))
            .transpose()?;
        if command
            .observation
            .active_at()
            .is_some_and(|active| active > command.now || previous_activity.is_some_and(|previous| active < previous))
        {
            return Err(HeartbeatError::InvalidActivity);
        }
        let deadline = command
            .now
            .checked_add_signed(ATTACHMENT_LEASE)
            .filter(|at| (-62_135_596_800..=253_402_300_799).contains(&at.timestamp()))
            .ok_or(HeartbeatError::LeaseDeadlineOverflow)?;
        if command.now.timestamp_subsec_nanos() >= 1_000_000_000 {
            return Err(HeartbeatError::InvalidState(EvolveError::TimestampRange {
                field: "attachment.observed_at",
            }));
        }
        let attachment = proto::PeerAttachment {
            attachment_ref: previous.attachment_ref.clone(),
            status: previous.status,
            observed_at: MessageField::some(timestamp_from_datetime(&command.now)),
            expires_at: MessageField::some(timestamp_from_datetime(&deadline)),
        };
        let active_at = command
            .observation
            .active_at()
            .or(previous_activity)
            .map(|at| MessageField::some(timestamp_from_datetime(&at)))
            .unwrap_or_default();
        Ok(Decision::event(PeerEvent {
            event: Some(
                proto::PeerHeartbeatRecorded {
                    attachment: MessageField::some(attachment),
                    availability: Some(command.observation.availability().into()),
                    condition: command.observation.condition().map(Into::into),
                    active_at,
                }
                .into(),
            ),
        }))
    }
}

/// Reads the live owner-bound attachment from an authentic peer history without deciding a write.
pub fn guard_live_attachment<'a>(
    state: &'a PeerState,
    owner: &TrustedPeerOwner,
    reference: &AttachmentRef,
    now: DateTime<Utc>,
) -> Result<&'a proto::PeerAttachment, HeartbeatError> {
    let enrollment = state.enrollment.as_option().ok_or(HeartbeatError::MissingEnrollment)?;
    let identity = state::parse_enrollment(enrollment)?;
    if identity.owner() != owner {
        return Err(HeartbeatError::OwnerMismatch);
    }
    let previous = state.attachment.as_option().ok_or(HeartbeatError::Inactive)?;
    state::validate_attachment(enrollment, previous)?;
    if reference.peer_id() != identity.peer_id()
        || previous.attachment_ref.connector_id.as_deref() != Some(reference.connector_id().as_str())
        || previous.attachment_ref.generation != Some(reference.generation())
    {
        return Err(HeartbeatError::Fenced);
    }
    if previous.status.and_then(|value| value.as_known()) != Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED) {
        return Err(HeartbeatError::Inactive);
    }
    let observed = state::read_time(previous.observed_at.as_option(), "attachment.observed_at")?;
    let expires = state::read_time(previous.expires_at.as_option(), "attachment.expires_at")?;
    if now < observed {
        return Err(HeartbeatError::ClockRegression);
    }
    if now >= expires {
        return Err(HeartbeatError::Expired);
    }
    Ok(previous)
}
