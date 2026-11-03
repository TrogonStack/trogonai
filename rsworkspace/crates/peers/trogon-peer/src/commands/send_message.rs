use super::domain::*;
use super::{state, EvolveError, MessageError};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};
#[allow(dead_code)]
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
    pub fn new(owner: TrustedPeerOwner, attachment: AttachmentRef, key: IdempotencyKey, message_id: MessageId, recipient: PeerId, collaboration: CollaborationRef, content: TrustedContent, reply_to: Option<MessageId>, now: DateTime<Utc>) -> Self {
        Self { owner, attachment, key, message_id, recipient, collaboration, content, reply_to, now }
    }
}
impl Decider for SendMessage {
    type StreamId = PeerId;
    type State = PeerState;
    type Event = PeerEvent;
    type DecideError = MessageError;
    type EvolveError = EvolveError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;
    fn stream_id(&self) -> &PeerId { self.attachment.peer_id() }
    fn initial_state() -> PeerState { state::initial_state() }
    fn evolve(state: PeerState, event: &PeerEvent) -> Result<PeerState, EvolveError> { state::evolve(state, event) }
    fn decide(_state: &PeerState, _command: &Self) -> Result<Decision<Self>, MessageError> { Err(MessageError::InvalidTransition) }
}
