use super::domain::*;
use super::{EvolveError, MessageError, state};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};
#[derive(Debug, Clone)]
pub struct AcknowledgeBatch {
    owner: TrustedPeerOwner,
    attachment: AttachmentRef,
    batch_id: BatchId,
    now: DateTime<Utc>,
}
impl AcknowledgeBatch {
    pub fn owner(&self) -> &TrustedPeerOwner {
        &self.owner
    }
    pub fn attachment(&self) -> &AttachmentRef {
        &self.attachment
    }
    pub fn batch_id(&self) -> &BatchId {
        &self.batch_id
    }
    pub fn new(owner: TrustedPeerOwner, attachment: AttachmentRef, batch_id: BatchId, now: DateTime<Utc>) -> Self {
        Self {
            owner,
            attachment,
            batch_id,
            now,
        }
    }
}
impl Decider for AcknowledgeBatch {
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
        let batch = state
            .batches
            .iter()
            .find(|b| b.batch_id.as_deref() == Some(command.batch_id.as_str()))
            .ok_or(MessageError::BatchNotFound)?;
        if batch.acknowledged_at.is_set() {
            return Err(MessageError::AlreadyAcknowledged(Box::new(batch.clone())));
        }
        if command.now < state::read_time(batch.prepared_at.as_option(), "prepared_at")? {
            return Err(MessageError::InvalidTransition);
        }
        Ok(Decision::event(PeerEvent {
            event: Some(
                trogonai_proto::peers::v1alpha1::PeerBatchAcknowledged {
                    batch_id: batch.batch_id.clone(),
                    acknowledged_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                        &command.now,
                    )),
                }
                .into(),
            ),
        }))
    }
}
