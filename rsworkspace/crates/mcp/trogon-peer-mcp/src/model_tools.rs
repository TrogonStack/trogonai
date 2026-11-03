use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use serde::de::DeserializeOwned;
use serde_json::Value;
use trogon_peer::{AttachmentRef, BatchId, IdempotencyKey, MessageId, PeerId};
use trogonai_proto::peers::v1alpha1 as v1;

use crate::connector_config::ConnectorConfig;
use crate::model_inputs::{AcknowledgeInput, RosterInput, SendInput, StatusInput};
use crate::operation_metadata::{acknowledgment_metadata, message_metadata, roster_metadata, timestamp_metadata};
use crate::peer_message_text::PeerMessageText;
use crate::{AcknowledgePeerBatch, PeerMcpError, ReadPeerMessageStatus, ReadPeerRoster, SendPeerMessage};

pub(crate) async fn roster<B: ReadPeerRoster>(
    backend: &B,
    request: CallToolRequestParams,
    attachment: &AttachmentRef,
) -> Result<CallToolResult, PeerMcpError> {
    let input: RosterInput = parse_input(request)?;
    let response = backend
        .roster(v1::RosterRequest {
            attachment: wire_attachment(attachment).into(),
            page_size: input.page_size,
            page_token: input.page_token,
        })
        .await
        .map_err(|error| PeerMcpError::Roster(Box::new(error)))?;
    metadata_result(roster_metadata(response)?)
}

pub(crate) async fn send<B: SendPeerMessage>(
    backend: &B,
    request: CallToolRequestParams,
    attachment: &AttachmentRef,
    config: &ConnectorConfig,
) -> Result<CallToolResult, PeerMcpError> {
    let input: SendInput = parse_input(request)?;
    let recipient = PeerId::parse(input.recipient_peer_id).map_err(PeerMcpError::InvalidModelInput)?;
    let key = IdempotencyKey::new(input.idempotency_key).map_err(PeerMcpError::InvalidModelInput)?;
    let text = PeerMessageText::new(input.text).map_err(PeerMcpError::InvalidModelInput)?;
    let reply = input
        .reply_to_message_id
        .map(MessageId::new)
        .transpose()
        .map_err(PeerMcpError::InvalidModelInput)?;
    let response = backend
        .send(v1::SendRequest {
            attachment: wire_attachment(attachment).into(),
            recipient_peer_id: Some(recipient.as_str().to_owned()),
            idempotency_key: Some(key.as_str().to_owned()),
            text: Some(text.as_str().to_owned()),
            reply_to_message_id: reply.map(|id| id.as_str().to_owned()),
        })
        .await
        .map_err(|error| PeerMcpError::Send(Box::new(error)))?;
    let metadata = response.message.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
    if metadata.recipient_peer_id.as_deref() != Some(recipient.as_str())
        || metadata.content_size_bytes.map(|size| size as usize) != Some(text.as_str().len())
    {
        return Err(PeerMcpError::InvalidMetadata);
    }
    metadata_result(message_metadata(Some(metadata), attachment, config)?)
}

pub(crate) async fn status<B: ReadPeerMessageStatus>(
    backend: &B,
    request: CallToolRequestParams,
    attachment: &AttachmentRef,
    config: &ConnectorConfig,
) -> Result<CallToolResult, PeerMcpError> {
    let input: StatusInput = parse_input(request)?;
    let key = IdempotencyKey::new(input.idempotency_key).map_err(PeerMcpError::InvalidModelInput)?;
    let response = backend
        .status(v1::StatusRequest {
            attachment: wire_attachment(attachment).into(),
            idempotency_key: Some(key.as_str().to_owned()),
        })
        .await
        .map_err(|error| PeerMcpError::Status(Box::new(error)))?;
    let mut output = message_metadata(response.message.as_option(), attachment, config)?;
    let status = response
        .status
        .as_ref()
        .and_then(|value| value.as_known())
        .ok_or(PeerMcpError::InvalidMetadata)?;
    if status == v1::MessageStatus::MESSAGE_STATUS_UNSPECIFIED {
        return Err(PeerMcpError::InvalidMetadata);
    }
    let object = output.as_object_mut().ok_or(PeerMcpError::InvalidMetadata)?;
    object.insert(
        "status".into(),
        serde_json::to_value(status).map_err(PeerMcpError::MetadataEncoding)?,
    );
    object.insert(
        "observed_at".into(),
        timestamp_metadata(response.observed_at.as_option())?,
    );
    metadata_result(output)
}

pub(crate) async fn acknowledge<B: AcknowledgePeerBatch>(
    backend: &B,
    request: CallToolRequestParams,
    attachment: &AttachmentRef,
) -> Result<CallToolResult, PeerMcpError> {
    let input: AcknowledgeInput = parse_input(request)?;
    let batch_id = BatchId::new(input.batch_id).map_err(PeerMcpError::InvalidModelInput)?;
    let response = backend
        .acknowledge_batch(v1::AcknowledgeBatchRequest {
            attachment: wire_attachment(attachment).into(),
            batch_id: Some(batch_id.as_str().to_owned()),
        })
        .await
        .map_err(|error| PeerMcpError::Acknowledge(Box::new(error)))?;
    metadata_result(acknowledgment_metadata(&response, &batch_id)?)
}

fn wire_attachment(attachment: &AttachmentRef) -> v1::AttachmentRef {
    v1::AttachmentRef {
        peer_id: Some(attachment.peer_id().as_str().to_owned()),
        connector_id: Some(attachment.connector_id().as_str().to_owned()),
        generation: Some(attachment.generation()),
    }
}

fn parse_input<T: DeserializeOwned>(request: CallToolRequestParams) -> Result<T, PeerMcpError> {
    serde_json::from_value(Value::Object(request.arguments.unwrap_or_default())).map_err(PeerMcpError::InvalidArguments)
}

fn metadata_result(value: Value) -> Result<CallToolResult, PeerMcpError> {
    let text = serde_json::to_string(&value).map_err(PeerMcpError::Encode)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}
