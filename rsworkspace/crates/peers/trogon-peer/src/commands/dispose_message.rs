use super::accepted_message::AcceptedMessage;
use super::domain::*;
use super::state::PeerState;
use super::{MessageError, state};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::v1alpha1::PeerEvent;
#[derive(Debug, Clone)]
pub struct DisposeMessage {
    pub(crate) evidence: AcceptedMessage,
    pub(crate) disposition: TerminalDisposition,
    pub(crate) now: DateTime<Utc>,
}
impl DisposeMessage {
    pub fn evidence(&self) -> &AcceptedMessage {
        &self.evidence
    }
    pub fn disposition(&self) -> TerminalDisposition {
        self.disposition
    }
    pub fn new(evidence: AcceptedMessage, disposition: TerminalDisposition, now: DateTime<Utc>) -> Self {
        Self {
            evidence,
            disposition,
            now,
        }
    }
}
impl Decider for DisposeMessage {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = MessageError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;
    fn stream_id(&self) -> &PeerId {
        &self.evidence.recipient
    }
    fn decide(state: &PeerState, command: &Self) -> Result<Decision<Self>, MessageError> {
        super::intake_guard::recipient_evidence(state, &command.evidence, command.now)?;
        if let Some(entry) = state
            .inbox
            .iter()
            .find(|e| e.message.message_id == command.evidence.message.message_id)
            && entry.message.as_option() != Some(&command.evidence.message)
        {
            return Err(MessageError::Conflict);
        }
        if let Some(receipt) = state
            .dispositions
            .iter()
            .find(|r| r.message_id == command.evidence.message.message_id)
        {
            return Err(MessageError::AlreadyDisposed(Box::new(receipt.clone())));
        }
        let expires = state::read_time(command.evidence.message.expires_at.as_option(), "expires_at")?;
        if command.disposition == TerminalDisposition::Expired && command.now < expires {
            return Err(MessageError::InvalidTransition);
        }
        let outcome = match command.disposition {
            TerminalDisposition::Expired => {
                trogonai_proto::peers::v1alpha1::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED
            }
            TerminalDisposition::Unavailable => {
                trogonai_proto::peers::v1alpha1::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE
            }
        };
        let receipt = super::message_rules::make_receipt(state, &command.evidence.message, outcome, None, command.now)?;
        Ok(Decision::event(PeerEvent {
            event: Some(
                trogonai_proto::peers::v1alpha1::PeerMessageDisposed {
                    receipt: buffa::MessageField::some(receipt),
                }
                .into(),
            ),
        }))
    }
}
