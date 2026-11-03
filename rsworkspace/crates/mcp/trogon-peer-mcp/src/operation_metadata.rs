use serde::Serialize;
use serde_json::Value;
use trogon_peer::{AttachmentRef, BatchId, MessageId, PeerDisplayName, PeerId};
use trogon_peer_context::{MAX_MESSAGE_BYTES, MAX_MESSAGES};
use trogonai_proto::peers::v1alpha1 as v1;

use crate::PeerMcpError;
use crate::acknowledgment_metadata_projection::AcknowledgmentMetadataProjection;
use crate::application_acknowledgment::ApplicationAcknowledgment;
use crate::connector_config::ConnectorConfig;
use crate::message_metadata_projection::MessageMetadataProjection;

pub(crate) fn message_metadata(
    metadata: Option<&v1::MessageMetadata>,
    attachment: &AttachmentRef,
    config: &ConnectorConfig,
) -> Result<MessageMetadataProjection, PeerMcpError> {
    let metadata = metadata.ok_or(PeerMcpError::InvalidMetadata)?;
    let message_id = metadata.message_id.as_deref().ok_or(PeerMcpError::InvalidMetadata)?;
    let message_id = MessageId::new(message_id).map_err(|_| PeerMcpError::InvalidMetadata)?;
    let sender = metadata
        .sender_peer_id
        .as_deref()
        .ok_or(PeerMcpError::InvalidMetadata)?;
    let recipient = metadata
        .recipient_peer_id
        .as_deref()
        .ok_or(PeerMcpError::InvalidMetadata)?;
    let recipient = PeerId::parse(recipient).map_err(|_| PeerMcpError::InvalidMetadata)?;
    if sender != attachment.peer_id().as_str()
        || metadata.collaboration_ref.as_deref() != Some(config.collaboration_ref().as_str())
        || !metadata
            .content_size_bytes
            .is_some_and(|size| (1..=MAX_MESSAGE_BYTES).contains(&(size as usize)))
    {
        return Err(PeerMcpError::InvalidMetadata);
    }
    let accepted = metadata.accepted_at.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
    let expires = metadata.expires_at.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
    if (expires.seconds, expires.nanos) <= (accepted.seconds, accepted.nanos) {
        return Err(PeerMcpError::InvalidMetadata);
    }
    Ok(MessageMetadataProjection {
        message_id: message_id.as_str().to_owned(),
        sender_peer_id: sender.to_owned(),
        recipient_peer_id: recipient.as_str().to_owned(),
        accepted_at: timestamp_metadata(Some(accepted))?,
        expires_at: timestamp_metadata(Some(expires))?,
    })
}

pub(crate) fn roster_metadata(response: v1::RosterResponse) -> Result<Value, PeerMcpError> {
    for peer in &response.peers {
        PeerId::parse(peer.peer_id.as_deref().ok_or(PeerMcpError::InvalidMetadata)?)
            .map_err(|_| PeerMcpError::InvalidMetadata)?;
        PeerDisplayName::new(peer.display_name.as_deref().ok_or(PeerMcpError::InvalidMetadata)?)
            .map_err(|_| PeerMcpError::InvalidMetadata)?;
        match peer.availability.as_ref().and_then(|value| value.as_known()) {
            Some(
                v1::PeerAvailability::PEER_AVAILABILITY_READY
                | v1::PeerAvailability::PEER_AVAILABILITY_BUSY
                | v1::PeerAvailability::PEER_AVAILABILITY_BLOCKED
                | v1::PeerAvailability::PEER_AVAILABILITY_DRAINING,
            ) => {}
            _ => return Err(PeerMcpError::InvalidMetadata),
        }
        if peer.condition.as_ref().is_some_and(|value| {
            value
                .as_known()
                .is_none_or(|known| known == v1::PeerCondition::PEER_CONDITION_UNSPECIFIED)
        }) || peer.delivery_capability.as_ref().and_then(|value| value.as_known())
            != Some(v1::DeliveryCapability::DELIVERY_CAPABILITY_ON_USER_PROMPT)
        {
            return Err(PeerMcpError::InvalidMetadata);
        }
        let observed = peer.observed_at.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
        let expires = peer.expires_at.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
        if (expires.seconds, expires.nanos) <= (observed.seconds, observed.nanos) {
            return Err(PeerMcpError::InvalidMetadata);
        }
        timestamp_metadata(Some(observed))?;
        timestamp_metadata(Some(expires))?;
        if let Some(active) = peer.active_at.as_option() {
            timestamp_metadata(Some(active))?;
        }
    }
    serde_json::to_value(response).map_err(PeerMcpError::MetadataEncoding)
}

pub(crate) fn acknowledgment_metadata(
    response: &v1::AcknowledgeBatchResponse,
    requested: &BatchId,
) -> Result<AcknowledgmentMetadataProjection, PeerMcpError> {
    let batch = response.batch.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
    if batch.batch_id.as_deref() != Some(requested.as_str())
        || batch.message_ids.is_empty()
        || batch.message_ids.len() > MAX_MESSAGES
    {
        return Err(PeerMcpError::InvalidMetadata);
    }
    for (index, id) in batch.message_ids.iter().enumerate() {
        MessageId::new(id).map_err(|_| PeerMcpError::InvalidMetadata)?;
        if batch.message_ids[..index].contains(id) {
            return Err(PeerMcpError::InvalidMetadata);
        }
    }
    let prepared = batch.prepared_at.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
    let acknowledged = batch.acknowledged_at.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
    if (acknowledged.seconds, acknowledged.nanos) < (prepared.seconds, prepared.nanos) {
        return Err(PeerMcpError::InvalidMetadata);
    }
    timestamp_metadata(Some(prepared))?;
    timestamp_metadata(Some(acknowledged))?;
    Ok(AcknowledgmentMetadataProjection {
        batch_id: requested.as_str().to_owned(),
        acknowledgment: ApplicationAcknowledgment::ApplicationAcknowledged,
    })
}

pub(crate) fn timestamp_metadata<T: Serialize>(value: Option<&T>) -> Result<String, PeerMcpError> {
    let value = value.ok_or(PeerMcpError::InvalidMetadata)?;
    let encoded = serde_json::to_value(value).map_err(PeerMcpError::MetadataEncoding)?;
    match encoded {
        Value::String(timestamp) => Ok(timestamp),
        _ => Err(PeerMcpError::InvalidMetadata),
    }
}
