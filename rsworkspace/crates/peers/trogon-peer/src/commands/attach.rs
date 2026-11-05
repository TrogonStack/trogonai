use buffa::MessageField;
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{
    state_v1alpha1::PeerState,
    v1alpha1::{self as proto, PeerEvent},
};

use super::constants::ATTACHMENT_LEASE;
use super::domain::{CollaborationRef, ConnectorId, EnrollmentIdentity, PeerId};
use super::state::{self, EvolveError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachError {
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
    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let enrollment = state.enrollment.as_option().ok_or(AttachError::MissingEnrollment)?;
        if state::parse_enrollment(enrollment)? != command.identity {
            return Err(AttachError::IdentityMismatch);
        }
        if state::required(&enrollment.collaboration_ref, "enrollment.collaboration_ref")?
            != command.collaboration.as_str()
        {
            return Err(AttachError::CollaborationMismatch);
        }
        let previous = state
            .attachment
            .as_option()
            .ok_or(EvolveError::MissingField { field: "attachment" })?;
        state::validate_attachment(enrollment, previous)?;
        let observed = state::read_time(previous.observed_at.as_option(), "attachment.observed_at")?;
        let expires = state::read_time(previous.expires_at.as_option(), "attachment.expires_at")?;
        if command.now < observed {
            return Err(AttachError::ClockRegression);
        }
        if previous.status.and_then(|status| status.as_known())
            == Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
            && command.now < expires
        {
            return Err(
                if previous.attachment.connector_id.as_deref() == Some(command.connector_id.as_str()) {
                    AttachError::AlreadyAttached
                } else {
                    AttachError::InUse
                },
            );
        }
        let generation = previous
            .attachment
            .generation
            .and_then(|generation| generation.checked_add(1))
            .ok_or(AttachError::GenerationOverflow)?;
        if command.now.timestamp_subsec_nanos() >= 1_000_000_000 {
            return Err(AttachError::LeaseDeadlineOverflow);
        }
        let expires_at = command
            .now
            .checked_add_signed(ATTACHMENT_LEASE)
            .filter(|at| state::timestamp_seconds_supported(at.timestamp()))
            .ok_or(AttachError::LeaseDeadlineOverflow)?;
        let attachment = proto::PeerAttachment {
            attachment: MessageField::some(proto::AttachmentRef {
                peer_id: Some(command.identity.peer_id().to_string()),
                connector_id: Some(command.connector_id.as_str().to_owned()),
                generation: Some(generation),
            }),
            status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED.into()),
            observed_at: MessageField::some(timestamp_from_datetime(&command.now)),
            expires_at: MessageField::some(timestamp_from_datetime(&expires_at)),
        };
        Ok(Decision::event(PeerEvent {
            event: Some(
                proto::PeerAttached {
                    attachment: MessageField::some(attachment),
                }
                .into(),
            ),
        }))
    }
}
