use serde_json::{Value, json};
use trogon_peer::{CollaborationRef, PeerId};
use trogonai_proto::peers::v1alpha1::PreparedMessage;

use crate::context_timestamp::ContextTimestamp;
use crate::{MAX_MESSAGE_BYTES, PeerContextError};

pub(crate) struct ContextMessage<'a> {
    message_id: &'a str,
    sender_peer_id: PeerId,
    recipient_peer_id: PeerId,
    collaboration_ref: CollaborationRef,
    accepted_at: ContextTimestamp<'a>,
    text: &'a str,
}

impl<'a> ContextMessage<'a> {
    pub(crate) fn from_wire(message: &'a PreparedMessage) -> Result<Self, PeerContextError> {
        let metadata = message
            .metadata
            .as_option()
            .ok_or(PeerContextError::ContractViolation)?;
        let message_id = parse_identifier(metadata.message_id.as_deref())?;
        let sender_peer_id = metadata
            .sender_peer_id
            .as_deref()
            .ok_or(PeerContextError::ContractViolation)?;
        let recipient_peer_id = metadata
            .recipient_peer_id
            .as_deref()
            .ok_or(PeerContextError::ContractViolation)?;
        let collaboration_ref = metadata
            .collaboration_ref
            .as_deref()
            .ok_or(PeerContextError::ContractViolation)?;
        let text = message.text.as_deref().ok_or(PeerContextError::ContractViolation)?;
        if text.len() > MAX_MESSAGE_BYTES || metadata.content_size_bytes.map(|size| size as usize) != Some(text.len()) {
            return Err(PeerContextError::ContractViolation);
        }
        let accepted_at = metadata
            .accepted_at
            .as_option()
            .ok_or(PeerContextError::ContractViolation)?;
        Ok(Self {
            message_id,
            sender_peer_id: PeerId::parse(sender_peer_id).map_err(|_| PeerContextError::ContractViolation)?,
            recipient_peer_id: PeerId::parse(recipient_peer_id).map_err(|_| PeerContextError::ContractViolation)?,
            collaboration_ref: CollaborationRef::new(collaboration_ref)
                .map_err(|_| PeerContextError::ContractViolation)?,
            accepted_at: ContextTimestamp::from_wire(accepted_at)?,
            text,
        })
    }

    pub(crate) fn message_id(&self) -> &str {
        self.message_id
    }

    pub(crate) fn recipient_peer_id(&self) -> &PeerId {
        &self.recipient_peer_id
    }

    pub(crate) fn collaboration_ref(&self) -> &CollaborationRef {
        &self.collaboration_ref
    }

    pub(crate) fn to_json(&self) -> Result<Value, PeerContextError> {
        Ok(json!({
            "message_id": self.message_id,
            "sender_peer_id": self.sender_peer_id.as_str(),
            "recipient_peer_id": self.recipient_peer_id.as_str(),
            "collaboration_ref": self.collaboration_ref.as_str(),
            "accepted_at": self.accepted_at.to_json()?,
            "text": self.text,
        }))
    }
}

pub(crate) fn parse_identifier(value: Option<&str>) -> Result<&str, PeerContextError> {
    match value {
        Some(value) if !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control) => Ok(value),
        _ => Err(PeerContextError::ContractViolation),
    }
}
