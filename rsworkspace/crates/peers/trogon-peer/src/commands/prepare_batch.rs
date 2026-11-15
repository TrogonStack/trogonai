use super::domain::*;
use super::{EvolveError, MessageError, state};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};
#[derive(Debug, Clone)]
pub struct PrepareBatch {
    owner: TrustedPeerOwner,
    attachment: AttachmentRef,
    batch_id: BatchId,
    plan: TrustedBatchPlan,
    now: DateTime<Utc>,
}
impl PrepareBatch {
    pub fn owner(&self) -> &TrustedPeerOwner {
        &self.owner
    }
    pub fn attachment(&self) -> &AttachmentRef {
        &self.attachment
    }
    pub fn batch_id(&self) -> &BatchId {
        &self.batch_id
    }
    pub fn plan(&self) -> &TrustedBatchPlan {
        &self.plan
    }
    pub fn new(
        owner: TrustedPeerOwner,
        attachment: AttachmentRef,
        batch_id: BatchId,
        plan: TrustedBatchPlan,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            owner,
            attachment,
            batch_id,
            plan,
            now,
        }
    }
}
impl Decider for PrepareBatch {
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
        if let Some(batch) = state.batches.iter().find(|b| !b.acknowledged_at.is_set()) {
            return Err(MessageError::AlreadyPrepared(Box::new(batch.clone())));
        }
        if state
            .batches
            .iter()
            .any(|b| b.batch_id.as_deref() == Some(command.batch_id.as_str()))
        {
            return Err(MessageError::Conflict);
        }
        let eligible = super::message_rules::eligible_inbox(state, command.now)?;
        let ids: Vec<_> = command
            .plan
            .message_ids
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        if !super::message_rules::valid_selection(&eligible, &ids) {
            return Err(MessageError::InvalidPlan);
        }
        let bytes: u64 = eligible
            .iter()
            .take(ids.len())
            .map(|e| u64::from(e.message.content_size_bytes.unwrap_or(0)))
            .sum();
        if bytes > command.plan.rendered_utf8_bytes as u64 {
            return Err(MessageError::InvalidPlan);
        }
        let batch_id = Some(command.batch_id.as_str().to_owned());
        let mut receipts = Vec::new();
        for (index, entry) in eligible.iter().take(ids.len()).enumerate() {
            let mut receipt = super::message_rules::make_receipt(
                state,
                &entry.message,
                trogonai_proto::peers::v1alpha1::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED,
                batch_id.clone(),
                command.now,
            )?;
            receipt.recipient_revision = receipt.recipient_revision.and_then(|n| n.checked_add(index as u64));
            receipts.push(receipt);
        }
        let batch = trogonai_proto::peers::v1alpha1::PeerBatch {
            batch_id,
            message_ids: ids,
            prepared_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&command.now)),
            ..Default::default()
        };
        Ok(Decision::event(PeerEvent {
            event: Some(
                trogonai_proto::peers::v1alpha1::PeerBatchPrepared {
                    batch: buffa::MessageField::some(batch),
                    receipts,
                }
                .into(),
            ),
        }))
    }
}
