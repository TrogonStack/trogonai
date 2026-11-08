use super::domain::*;
use super::{EvolveError, MessageError, state};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};
#[derive(Debug, Clone)]
pub struct SendMessage {
    pub(crate) owner: TrustedPeerOwner,
    pub(crate) attachment: AttachmentRef,
    pub(crate) key: IdempotencyKey,
    pub(crate) message_id: MessageId,
    pub(crate) recipient: PeerId,
    pub(crate) collaboration: CollaborationRef,
    pub(crate) content: TrustedContent,
    pub(crate) reply_to: Option<MessageId>,
    pub(crate) now: DateTime<Utc>,
}
impl SendMessage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner: TrustedPeerOwner,
        attachment: AttachmentRef,
        key: IdempotencyKey,
        message_id: MessageId,
        recipient: PeerId,
        collaboration: CollaborationRef,
        content: TrustedContent,
        reply_to: Option<MessageId>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            owner,
            attachment,
            key,
            message_id,
            recipient,
            collaboration,
            content,
            reply_to,
            now,
        }
    }
}
impl Decider for SendMessage {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = MessageError;
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
    fn decide(state: &PeerState, command: &Self) -> Result<Decision<Self>, MessageError> {
        super::heartbeat::guard_live_attachment(state, &command.owner, &command.attachment, command.now)?;
        let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
        let mut message = trogonai_proto::peers::v1alpha1::MessageMetadata {
            message_id: Some(command.message_id.as_str().to_owned()),
            sender_peer_id: Some(command.attachment.peer_id().to_string()),
            recipient_peer_id: Some(command.recipient.to_string()),
            collaboration_ref: Some(command.collaboration.as_str().to_owned()),
            content_ref: command.content.reference.to_claim_check().into(),
            content_size_bytes: Some(command.content.size),
            semantic_fingerprint: command.content.semantic_fingerprint.to_wire().into(),
            reply_to_message_id: command.reply_to.as_ref().map(|id| id.as_str().to_owned()),
            ..Default::default()
        };
        let digest = super::message_rules::semantic_digest(&message)?;
        for old in &state.outbox {
            super::message_rules::outbox_entry(old)?;
            if old.idempotency_key.as_deref() == Some(command.key.as_str()) {
                return if old.semantic_digest.as_deref() == Some(digest.as_slice()) {
                    Err(MessageError::AlreadyAccepted(Box::new(old.clone())))
                } else {
                    Err(MessageError::Conflict)
                };
            }
        }
        if enrollment.collaboration_ref.as_deref() != Some(command.collaboration.as_str()) {
            return Err(MessageError::CollaborationMismatch);
        }
        let expires = command
            .now
            .checked_add_signed(chrono::Duration::hours(1))
            .ok_or(EvolveError::InvalidMessageTransition)?;
        message.accepted_at = buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&command.now));
        message.expires_at = buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&expires));
        super::message_rules::metadata(&message)?;
        if state
            .outbox
            .iter()
            .any(|old| old.message.message_id == message.message_id)
        {
            return Err(MessageError::Conflict);
        }
        let pending: Vec<_> = state.outbox.iter().filter(|entry| !entry.settlement.is_set()).collect();
        let bytes: u64 = pending
            .iter()
            .map(|entry| u64::from(entry.message.content_size_bytes.unwrap_or(0)))
            .sum();
        if pending.len() >= 16 || bytes + u64::from(command.content.size) > 32768 {
            return Err(MessageError::SenderCapacity);
        }
        let mut recent = 0;
        for entry in &state.outbox {
            let accepted = state::read_time(entry.message.accepted_at.as_option(), "accepted_at")?;
            if accepted > command.now {
                return Err(MessageError::Guard(super::HeartbeatError::ClockRegression));
            }
            if command.now - accepted < chrono::Duration::seconds(60) {
                recent += 1;
            }
        }
        if recent >= 10 {
            return Err(MessageError::RateLimited);
        }
        Ok(Decision::event(PeerEvent {
            event: Some(
                trogonai_proto::peers::v1alpha1::PeerMessageAccepted {
                    entry: buffa::MessageField::some(trogonai_proto::peers::v1alpha1::OutboxEntry {
                        idempotency_key: Some(command.key.as_str().to_owned()),
                        semantic_digest: Some(digest),
                        message: buffa::MessageField::some(message),
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        }))
    }
}
