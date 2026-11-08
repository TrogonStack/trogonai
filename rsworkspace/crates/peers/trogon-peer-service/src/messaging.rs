use crate::{
    attachment_input::attachment,
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
use trogon_peer::{IdempotencyKey, MessageError, MessageId, PeerId, SendMessage, TrustedPeerOwner};
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
        let collaboration = self.collaboration(&sender)?;
        let target = self.load(owner, &recipient).await?;
        if self.collaboration(&target)? != collaboration {
            return Err(PeerApplicationError::Scope);
        }
        let binding = self.content.seal(&scope, &text).await?;
        if binding.tenant() != owner.tenant() || binding.content().size() as usize != text.as_str().len() {
            return Err(PeerApplicationError::Scope);
        }
        let command = SendMessage::new(
            owner.clone(),
            reference,
            key.clone(),
            self.ids.message_id()?,
            recipient,
            collaboration,
            binding.content().clone(),
            reply,
            self.clock.now(),
        );
        let message = match self.execute_message(owner, &command).await {
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
        let entry = state
            .outbox
            .iter()
            .find(|e| e.idempotency_key.as_deref() == Some(key.as_str()))
            .ok_or(MessageError::MessageNotFound)?;
        let recipient = PeerId::parse(
            entry
                .message
                .recipient_peer_id
                .clone()
                .ok_or(PeerApplicationError::Scope)?,
        )?;
        self.authorize(owner, &recipient, PeerOperation::ReadPeer)?;
        let target = self.load(owner, &recipient).await?;
        if self.collaboration(&target)? != self.collaboration(&state)? {
            return Err(PeerApplicationError::Scope);
        }
        let receipt = target
            .dispositions
            .iter()
            .find(|r| r.message_id == entry.message.message_id);
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
                .any(|e| e.message.message_id == entry.message.message_id) =>
            {
                proto::MessageStatus::MESSAGE_STATUS_INTAKE_RECORDED
            }
            _ => proto::MessageStatus::MESSAGE_STATUS_ACCEPTED,
        };
        Ok(proto::StatusResponse {
            message: entry.message.clone(),
            status: Some(status.into()),
            observed_at: MessageField::some(timestamp_from_datetime(&self.clock.now())),
        })
    }
}
