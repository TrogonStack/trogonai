use crate::{
    attachment_input::attachment,
    content_ports::ContentOpen,
    identity_ports::BatchIdGenerator,
    peer_access::PeerAccess,
    peer_application::{ApplicationResult, PeerApplication},
    peer_application_error::PeerApplicationError,
    peer_clock::PeerClock,
    peer_operation::PeerOperation,
    peer_port_error::PeerPortError,
    prepared_presentation::PreparedPresentation,
    presentation_key::PresentationKey,
    presentation_ports::{PresentationInsert, PresentationOrphan, PresentationRead},
    prompt_id::PromptId,
    reconciliation::binding,
    scoped_peer_store::ScopedPeerStore,
};
use buffa::MessageField;
use trogon_decider_runtime::{CommandAuthorizer, CommandError, StreamAppend, StreamRead};
use trogon_peer::{BatchId, MessageError, MessageId, PeerId, PrepareBatch, TrustedBatchPlan, TrustedPeerOwner};
use trogon_peer_context::{MAX_MESSAGES, PeerContext, PeerContextError};
use trogonai_proto::{
    convert::{datetime_from_timestamp, timestamp_from_datetime},
    peers::{state_v1alpha1::PeerState, v1alpha1 as proto},
};
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: CommandAuthorizer<PeerAccess> + CommandAuthorizer<PrepareBatch> + Send + Sync,
    C: PeerClock,
    Content: ContentOpen,
    Cache: PresentationRead + PresentationInsert + PresentationOrphan,
    Ids: BatchIdGenerator,
{
    pub async fn prepare_batch(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::PrepareBatchRequest,
    ) -> ApplicationResult<proto::PrepareBatchResponse, S> {
        let reference = attachment(request.attachment.as_option().ok_or(PeerApplicationError::Input)?)?;
        let _prompt = request
            .prompt_id
            .filter(|value| !value.is_empty())
            .map(PromptId::new)
            .transpose()?;
        let scope = self.authorize(owner, reference.peer_id(), PeerOperation::Prepare)?;
        let state = self.load(owner, reference.peer_id()).await?;
        self.live(owner, &reference, &state)?;
        if let Some(batch) = state.batches.iter().find(|b| !b.acknowledged_at.is_set()) {
            return self.replay(owner, reference.peer_id(), &state, batch).await;
        }
        let now = self.clock.now();
        let mut eligible = Vec::new();
        for entry in &state.inbox {
            let accepted = datetime_from_timestamp(
                entry
                    .message
                    .accepted_at
                    .as_option()
                    .ok_or(PeerApplicationError::Scope)?,
            )
            .map_err(|_| PeerApplicationError::Scope)?;
            let expires = datetime_from_timestamp(
                entry
                    .message
                    .expires_at
                    .as_option()
                    .ok_or(PeerApplicationError::Scope)?,
            )
            .map_err(|_| PeerApplicationError::Scope)?;
            if !entry.disposition.is_set() && accepted <= now && now < expires {
                eligible.push(entry);
            }
        }
        eligible.sort_by_key(|e| e.intake_ordinal);
        if eligible.is_empty() {
            return Ok(response(
                None,
                Vec::new(),
                proto::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE,
            ));
        }
        let batch_id = self.ids.batch_id()?;
        let mut batch = proto::PeerBatch {
            batch_id: Some(batch_id.as_str().to_owned()),
            prepared_at: MessageField::some(timestamp_from_datetime(&now)),
            ..Default::default()
        };
        let mut messages = Vec::new();
        let mut frame = None;
        for entry in eligible.into_iter().take(MAX_MESSAGES) {
            let metadata = entry.message.as_option().ok_or(PeerApplicationError::Scope)?;
            let content = binding(owner, metadata)?;
            let Some(text) = self.content.open(&scope, &content).await? else {
                if messages.is_empty() {
                    return Ok(response(
                        None,
                        Vec::new(),
                        proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE,
                    ));
                }
                break;
            };
            if text.as_str().len() != content.content().size() as usize {
                return Err(PeerApplicationError::Scope);
            }
            batch
                .message_ids
                .push(metadata.message_id.clone().ok_or(PeerApplicationError::Scope)?);
            messages.push(proto::PreparedMessage {
                metadata: MessageField::some(metadata.clone()),
                text: Some(text.as_str().to_owned()),
            });
            match PeerContext::render(&batch, &messages) {
                Ok(rendered) => frame = Some(rendered),
                Err(PeerContextError::ContractViolation) if messages.len() > 1 => {
                    messages.pop();
                    batch.message_ids.pop();
                    break;
                }
                Err(error) => return Err(PeerApplicationError::Context(error)),
            }
        }
        let frame = frame.ok_or(PeerApplicationError::Scope)?;
        let plan = TrustedBatchPlan::from_renderer(
            batch
                .message_ids
                .iter()
                .cloned()
                .map(MessageId::new)
                .collect::<Result<Vec<_>, _>>()?,
            frame.as_str().len(),
        )?;
        let key = PresentationKey::new(owner.tenant().clone(), reference.peer_id().clone(), batch_id.clone());
        let record = PreparedPresentation::from_port(batch.clone(), messages, frame.as_str().to_owned());
        self.cache.insert(&key, &record).await?;
        let command = PrepareBatch::new(owner.clone(), reference.clone(), batch_id, plan, now);
        match self.execute_message(owner, &command).await {
            Ok(winner) => {
                let batch = winner
                    .batches
                    .iter()
                    .find(|b| !b.acknowledged_at.is_set())
                    .ok_or(PeerApplicationError::Scope)?;
                self.replay(owner, reference.peer_id(), &winner, batch).await
            }
            Err(CommandError::Decide(MessageError::AlreadyPrepared(_))) => {
                self.cache.orphan(&key).await?;
                let winner = self.load(owner, reference.peer_id()).await?;
                self.live(owner, &reference, &winner)?;
                let batch = winner
                    .batches
                    .iter()
                    .find(|b| !b.acknowledged_at.is_set())
                    .ok_or(PeerApplicationError::Scope)?;
                self.replay(owner, reference.peer_id(), &winner, batch).await
            }
            Err(CommandError::Decide(error)) => {
                self.cache.orphan(&key).await?;
                Err(PeerApplicationError::Message(CommandError::Decide(error)))
            }
            Err(error) => Err(PeerApplicationError::Message(error)),
        }
    }
    async fn replay(
        &self,
        owner: &TrustedPeerOwner,
        peer: &PeerId,
        state: &PeerState,
        batch: &proto::PeerBatch,
    ) -> ApplicationResult<proto::PrepareBatchResponse, S> {
        let key = PresentationKey::new(
            owner.tenant().clone(),
            peer.clone(),
            BatchId::new(batch.batch_id.clone().ok_or(PeerApplicationError::Scope)?)?,
        );
        let cached = match self.cache.read(&key).await {
            Ok(record) => record,
            Err(PeerPortError::Unavailable) => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(record) = cached {
            let metadata_matches = record.messages().iter().all(|m| {
                state.inbox.iter().any(|entry| {
                    entry.message == m.metadata && entry.message.recipient_peer_id.as_deref() == Some(peer.as_str())
                })
            });
            if record.batch() == batch
                && metadata_matches
                && PeerContext::render(batch, record.messages()).is_ok_and(|frame| frame.as_str() == record.frame())
            {
                return Ok(response(
                    Some(batch.clone()),
                    record.messages().to_vec(),
                    proto::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE,
                ));
            }
        }
        Ok(response(
            Some(batch.clone()),
            Vec::new(),
            proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE,
        ))
    }
}
fn response(
    batch: Option<proto::PeerBatch>,
    messages: Vec<proto::PreparedMessage>,
    availability: proto::ContentAvailability,
) -> proto::PrepareBatchResponse {
    proto::PrepareBatchResponse {
        batch: batch.map(MessageField::some).unwrap_or_default(),
        messages,
        content_availability: Some(availability.into()),
    }
}
