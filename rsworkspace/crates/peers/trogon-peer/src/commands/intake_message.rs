use super::domain::*;
use super::{EvolveError, MessageError, state};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};
#[derive(Debug, Clone)]
pub struct IntakeMessage {
    pub(crate) evidence: AcceptedMessage,
    pub(crate) now: DateTime<Utc>,
}
impl IntakeMessage {
    pub fn new(evidence: AcceptedMessage, now: DateTime<Utc>) -> Self {
        Self { evidence, now }
    }
}
impl Decider for IntakeMessage {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = MessageError;
    type EvolveError = EvolveError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;
    fn stream_id(&self) -> &PeerId {
        &self.evidence.recipient
    }
    fn initial_state() -> PeerState {
        state::initial_state()
    }
    fn evolve(state: PeerState, event: &PeerEvent) -> Result<PeerState, EvolveError> {
        state::evolve(state, event)
    }
    fn decide(state: &PeerState, command: &Self) -> Result<Decision<Self>, MessageError> {
        super::message_rules::recipient_evidence(state, &command.evidence, command.now)?;
        super::message_rules::prior_intake(state, &command.evidence)?;
        let expires = state::read_time(command.evidence.message.expires_at.as_option(), "expires_at")?;
        if command.now >= expires {
            return Err(MessageError::RequiresDisposition);
        }
        let (count, bytes) = super::message_rules::pending_inbox(state)?;
        if count >= 64 || bytes + u64::from(command.evidence.message.content_size_bytes.unwrap_or(0)) > 131072 {
            return Err(MessageError::DeferIntake);
        }
        let ordinal = state
            .next_intake_ordinal
            .filter(|n| *n > 0 && *n < u64::MAX)
            .ok_or(EvolveError::InvalidMessageTransition)?;
        Ok(Decision::event(PeerEvent {
            event: Some(
                trogonai_proto::peers::v1alpha1::PeerMessageIntakeRecorded {
                    entry: buffa::MessageField::some(trogonai_proto::peers::v1alpha1::InboxEntry {
                        message: buffa::MessageField::some(command.evidence.message.clone()),
                        intake_ordinal: Some(ordinal),
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        }))
    }
}
