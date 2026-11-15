use crate::{
    attachment_input::attachment,
    constants::{MAX_BATCH_ID_BYTES, MAX_TIMESTAMP_NANOS},
    content_ports::ContentSeal,
    identity_ports::MessageIdGenerator,
    message_text::MessageText,
    peer_access::PeerAccess,
    peer_application::{ApplicationResult, PeerApplication},
    peer_application_error::PeerApplicationError,
    peer_clock::PeerClock,
    peer_operation::PeerOperation,
    scoped_peer_store::ScopedPeerStore,
};
use buffa::MessageField;
use trogon_decider_runtime::{CommandAuthorizer, CommandError, StreamAppend, StreamRead};
use trogon_peer::{BatchId, IdempotencyKey, MessageError, MessageId, PeerId, SendMessage, TrustedPeerOwner};
use trogon_peer_context::PeerContext;
use trogonai_proto::{convert::timestamp_from_datetime, peers::v1alpha1 as proto};
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: CommandAuthorizer<PeerAccess> + CommandAuthorizer<SendMessage> + Send + Sync,
    C: PeerClock,
    Content: ContentSeal,
    Ids: MessageIdGenerator,
{
    pub async fn send(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::SendRequest,
    ) -> ApplicationResult<proto::SendResponse, S> {
        let reference = attachment(request.attachment_ref.as_option().ok_or(PeerApplicationError::Input)?)?;
        let recipient = PeerId::parse(request.recipient_peer_id.ok_or(PeerApplicationError::Input)?)?;
        let key = IdempotencyKey::new(request.idempotency_key.ok_or(PeerApplicationError::Input)?)?;
        let text = MessageText::new(request.text.ok_or(PeerApplicationError::Input)?)?;
        let reply = request.reply_to_message_id.map(MessageId::new).transpose()?;
        let scope = self.authorize(owner, reference.peer_id(), PeerOperation::Send)?;
        self.authorize(owner, &recipient, PeerOperation::ReadPeer)?;
        let sender = self.load(owner, reference.peer_id()).await?;
        self.live(owner, &reference, &sender)?;
        let collaboration = sender.collaboration();
        let target = self.load(owner, &recipient).await?;
        if target.collaboration() != collaboration {
            return Err(PeerApplicationError::Scope);
        }
        let message_id = self.ids.message_id()?;
        if !sender
            .outbox
            .iter()
            .any(|entry| entry.idempotency_key.as_deref() == Some(key.as_str()))
        {
            let batch_id = BatchId::new("b".repeat(MAX_BATCH_ID_BYTES))?;
            let admission_time = self.clock.now();
            if admission_time.timestamp_subsec_nanos() >= 1_000_000_000 {
                return Err(PeerApplicationError::Evidence(trogon_peer::MessageError::InvalidState(
                    trogon_peer::EvolveError::TimestampRange { field: "accepted_at" },
                )));
            }
            let mut admission_at = timestamp_from_datetime(&admission_time);
            admission_at.nanos = MAX_TIMESTAMP_NANOS;
            let batch = proto::PeerBatch {
                batch_id: Some(batch_id.as_str().to_owned()),
                message_ids: vec![message_id.as_str().to_owned()],
                prepared_at: MessageField::some(admission_at.clone()),
                ..Default::default()
            };
            let message = proto::PreparedMessage {
                metadata: MessageField::some(proto::MessageMetadata {
                    message_id: Some(message_id.as_str().to_owned()),
                    sender_peer_id: Some(reference.peer_id().to_string()),
                    recipient_peer_id: Some(recipient.to_string()),
                    collaboration_ref: Some(collaboration.as_str().to_owned()),
                    content_size_bytes: Some(text.as_str().len() as u32),
                    accepted_at: MessageField::some(admission_at),
                    ..Default::default()
                }),
                text: Some(text.as_str().to_owned()),
            };
            PeerContext::render(&batch, &[message])?;
        }
        let binding = self.content.seal(&scope, &text).await?;
        if binding.tenant() != owner.tenant() || binding.content().size() as usize != text.as_str().len() {
            return Err(PeerApplicationError::Scope);
        }
        let sender = self.load(owner, reference.peer_id()).await?;
        let authorized_at = self.live(owner, &reference, &sender)?;
        let message = match self
            .execute_message_with_builder(owner, authorized_at, |accepted_at| {
                SendMessage::new(
                    owner.clone(),
                    reference.clone(),
                    key.clone(),
                    message_id.clone(),
                    recipient.clone(),
                    collaboration.clone(),
                    binding.content().clone(),
                    reply.clone(),
                    accepted_at,
                )
            })
            .await
        {
            Ok(state) => state
                .outbox
                .iter()
                .find(|e| e.idempotency_key.as_deref() == Some(key.as_str()))
                .and_then(|e| e.message.as_option())
                .cloned()
                .ok_or(PeerApplicationError::Scope)?,
            Err(PeerApplicationError::Message(CommandError::Decide(MessageError::AlreadyAccepted(entry)))) => {
                entry.message.as_option().cloned().ok_or(PeerApplicationError::Scope)?
            }
            Err(error) => return Err(error),
        };
        Ok(proto::SendResponse {
            message: MessageField::some(message),
        })
    }
    pub async fn status(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::StatusRequest,
    ) -> ApplicationResult<proto::StatusResponse, S> {
        let reference = attachment(request.attachment_ref.as_option().ok_or(PeerApplicationError::Input)?)?;
        let key = IdempotencyKey::new(request.idempotency_key.ok_or(PeerApplicationError::Input)?)?;
        self.authorize(owner, reference.peer_id(), PeerOperation::Status)?;
        let state = self.load(owner, reference.peer_id()).await?;
        self.live(owner, &reference, &state)?;
        let evidence = state.accepted(&key).ok_or(MessageError::MessageNotFound)?;
        let recipient = evidence.recipient();
        self.authorize(owner, recipient, PeerOperation::ReadPeer)?;
        let target = self.load(owner, recipient).await?;
        if target.collaboration() != state.collaboration() {
            return Err(PeerApplicationError::Scope);
        }
        let receipt = target
            .dispositions
            .iter()
            .find(|r| r.message_id == evidence.metadata().message_id);
        let status = match receipt.and_then(|r| r.outcome).and_then(|v| v.as_known()) {
            Some(proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED) => {
                if target
                    .batches
                    .iter()
                    .any(|b| receipt.is_some_and(|r| r.batch_id == b.batch_id) && b.acknowledged_at.is_set())
                {
                    proto::MessageStatus::MESSAGE_STATUS_APPLICATION_ACKNOWLEDGED
                } else {
                    proto::MessageStatus::MESSAGE_STATUS_BATCH_PREPARED
                }
            }
            Some(proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED) => {
                proto::MessageStatus::MESSAGE_STATUS_EXPIRED
            }
            Some(proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE) => {
                proto::MessageStatus::MESSAGE_STATUS_UNAVAILABLE
            }
            _ if target
                .inbox
                .iter()
                .any(|e| e.message.message_id == evidence.metadata().message_id) =>
            {
                proto::MessageStatus::MESSAGE_STATUS_INTAKE_RECORDED
            }
            _ => proto::MessageStatus::MESSAGE_STATUS_ACCEPTED,
        };
        let observed_at = self.clock.now();
        if !(-62_135_596_800..=253_402_300_799).contains(&observed_at.timestamp())
            || observed_at.timestamp_subsec_nanos() >= 1_000_000_000
        {
            return Err(PeerApplicationError::Evidence(trogon_peer::MessageError::InvalidState(
                trogon_peer::EvolveError::TimestampRange {
                    field: "status.observed_at",
                },
            )));
        }
        Ok(proto::StatusResponse {
            message: MessageField::some(evidence.metadata().clone()),
            status: Some(status.into()),
            observed_at: MessageField::some(timestamp_from_datetime(&observed_at)),
        })
    }
}
