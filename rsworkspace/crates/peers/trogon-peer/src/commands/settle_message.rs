use super::authoritative_receipt::AuthoritativeReceipt;
use super::domain::*;
use super::{EvolveError, MessageError, state};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};
#[derive(Debug, Clone)]
pub struct SettleMessage {
    sender: PeerId,
    evidence: AuthoritativeReceipt,
}
impl SettleMessage {
    pub fn new(sender: PeerId, evidence: AuthoritativeReceipt) -> Self {
        Self { sender, evidence }
    }
}
impl Decider for SettleMessage {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = MessageError;
    type EvolveError = EvolveError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;
    fn stream_id(&self) -> &PeerId {
        &self.sender
    }
    fn initial_state() -> PeerState {
        state::initial_state()
    }
    fn evolve(state: PeerState, event: &PeerEvent) -> Result<PeerState, EvolveError> {
        state::evolve(state, event)
    }
    fn decide(state: &PeerState, command: &Self) -> Result<Decision<Self>, MessageError> {
        let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
        let identity = state::parse_enrollment(enrollment)?;
        if identity.peer_id() != &command.sender {
            return Err(MessageError::RecipientMismatch);
        }
        if identity.owner().tenant() != &command.evidence.tenant {
            return Err(MessageError::TenantMismatch);
        }
        let receipt = &command.evidence.receipt;
        let entry = state
            .outbox
            .iter()
            .find(|e| e.message.message_id == receipt.message_id)
            .ok_or(MessageError::MessageNotFound)?;
        super::message_rules::outbox_entry(entry)?;
        super::message_rules::receipt_matches_message(receipt, &entry.message)?;
        if let Some(old) = entry.settlement.as_option() {
            return if old == receipt {
                Err(MessageError::AlreadySettled(Box::new(old.clone())))
            } else {
                Err(MessageError::Conflict)
            };
        }
        Ok(Decision::event(PeerEvent {
            event: Some(
                trogonai_proto::peers::v1alpha1::PeerMessageSettled {
                    receipt: buffa::MessageField::some(receipt.clone()),
                }
                .into(),
            ),
        }))
    }
}
