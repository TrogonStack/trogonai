use buffa::MessageField;
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, Events, WritePrecondition};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{
    state_v1alpha1::PeerState,
    v1alpha1::{self as proto, PeerEvent},
};

use super::domain::{CollaborationRef, ConnectorId, EnrollmentIdentity, PeerDisplayName, PeerId};
use super::state::{self, EvolveError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnrollError {
    #[error("peer communication identity is already enrolled")]
    AlreadyEnrolled,
    #[error("attachment lease deadline is outside the supported timestamp range")]
    LeaseDeadlineOverflow,
}

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
        Self {
            identity,
            connector_id,
            collaboration,
            display_name,
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

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::NoStream;

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
        if state.enrollment.is_set() {
            return Err(EnrollError::AlreadyEnrolled);
        }
        let expires_at = command
            .now
            .checked_add_signed(chrono::Duration::seconds(10))
            .filter(|at| (-62_135_596_800..=253_402_300_799).contains(&at.timestamp()))
            .ok_or(EnrollError::LeaseDeadlineOverflow)?;
        if !(-62_135_596_800..=253_402_300_799).contains(&command.now.timestamp()) {
            return Err(EnrollError::LeaseDeadlineOverflow);
        }
        let identity = &command.identity;
        let enrollment = proto::PeerEnrollment {
            peer_id: Some(identity.peer_id().as_str().to_owned()),
            tenant_id: Some(identity.owner().tenant().as_str().to_owned()),
            caller_id: Some(identity.owner().caller().as_str().to_owned()),
            installation_id: Some(identity.installation().as_str().to_owned()),
            native_session_id: Some(identity.native_session().as_str().to_owned()),
            collaboration_ref: Some(command.collaboration.as_str().to_owned()),
            display_name: Some(command.display_name.as_str().to_owned()),
            enrolled_at: MessageField::some(timestamp_from_datetime(&command.now)),
        };
        let attachment = proto::PeerAttachment {
            attachment: MessageField::some(proto::AttachmentRef {
                peer_id: Some(identity.peer_id().as_str().to_owned()),
                connector_id: Some(command.connector_id.as_str().to_owned()),
                generation: Some(1),
            }),
            status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED.into()),
            observed_at: MessageField::some(timestamp_from_datetime(&command.now)),
            expires_at: MessageField::some(timestamp_from_datetime(&expires_at)),
        };
        Ok(Decision::events(Events::from_first(
            PeerEvent {
                event: Some(
                    proto::PeerEnrolled {
                        enrollment: MessageField::some(enrollment),
                    }
                    .into(),
                ),
            },
            vec![PeerEvent {
                event: Some(
                    proto::PeerAttached {
                        attachment: MessageField::some(attachment),
                    }
                    .into(),
                ),
            }],
        )))
    }
}
