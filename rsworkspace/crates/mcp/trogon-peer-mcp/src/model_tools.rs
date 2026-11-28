use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::sync::Mutex;
use trogon_peer::{AttachmentRef, BatchId, IdempotencyKey, MessageId, PeerId, PeerValueError};
use trogonai_proto::peers::v1alpha1 as v1;

use crate::argument_issue::ArgumentIssue;
use crate::connector_config::ConnectorConfig;
use crate::constants::{DIRECTORY_BUDGET, DIRECTORY_PAGE_SIZE};
use crate::model_inputs::{AcknowledgeInput, RosterInput, SendInput, StatusInput};
use crate::model_tool_spec::ModelToolSpec;
use crate::model_tool_spec::constants::{ACKNOWLEDGE, ROSTER, SEND, STATUS};
use crate::operation_metadata::{acknowledgment_metadata, message_metadata, roster_metadata, timestamp_metadata};
use crate::peer_directory::PeerDirectory;
use crate::peer_message_text::PeerMessageText;
use crate::roster_page::RosterPage;
use crate::status_metadata_projection::StatusMetadataProjection;
use crate::{
    AcknowledgePeerBatch, PeerMcpError, ReadPeerMessageStatus, ReadPeerRoster, SendPeerMessage, ToolArgumentError,
};

pub(crate) async fn roster<B: ReadPeerRoster>(
    backend: &B,
    request: CallToolRequestParams,
    attachment: &AttachmentRef,
    directory: &Mutex<PeerDirectory>,
) -> Result<CallToolResult, PeerMcpError> {
    let input: RosterInput = parse_input(request, &ROSTER)?;
    let input_was_first_page = input.page_token.is_none();
    let response = backend
        .roster(v1::RosterRequest {
            attachment_ref: wire_attachment(attachment).into(),
            page_size: input.page_size,
            page_token: input.page_token,
        })
        .await
        .map_err(|error| PeerMcpError::Roster(Box::new(error)))?;
    let requested = if input_was_first_page {
        RosterPage::Whole
    } else {
        RosterPage::Partial
    };
    directory.lock().await.observe(&response, requested);
    metadata_result(roster_metadata(response)?)
}

/// Reads the first roster page into `directory`, returning how much of the roster it covered.
pub(crate) async fn refresh_directory<B: ReadPeerRoster>(
    backend: &B,
    attachment: &AttachmentRef,
    directory: &Mutex<PeerDirectory>,
) -> Option<RosterPage> {
    let response = backend
        .roster(v1::RosterRequest {
            attachment_ref: wire_attachment(attachment).into(),
            page_size: Some(DIRECTORY_PAGE_SIZE),
            page_token: None,
        })
        .await
        .ok()?;
    Some(directory.lock().await.observe(&response, RosterPage::Whole))
}

pub(crate) async fn send<B: SendPeerMessage + ReadPeerRoster>(
    backend: &B,
    request: CallToolRequestParams,
    attachment: &AttachmentRef,
    config: &ConnectorConfig,
    directory: &Mutex<PeerDirectory>,
) -> Result<CallToolResult, PeerMcpError> {
    let input: SendInput = parse_input(request, &SEND)?;
    let own = attachment.peer_id();
    let recipient = match PeerId::parse(input.recipient_peer_id.as_str()) {
        Ok(recipient) => recipient,
        Err(source) => {
            let issue = ArgumentIssue::MalformedRecipient {
                characters: input.recipient_peer_id.chars().count(),
                source,
            };
            let recipients = directory.lock().await.recipients(own);
            return Err(PeerMcpError::InvalidModelInput(
                ToolArgumentError::new(&SEND, issue).with_recipients(recipients),
            ));
        }
    };
    if &recipient == own {
        let recipients = directory.lock().await.recipients(own);
        return Err(PeerMcpError::OwnRecipient {
            own: recipient,
            recipients,
        });
    }
    let key = IdempotencyKey::new(input.idempotency_key).map_err(invalid_value(
        &SEND,
        "idempotency_key",
        "is your own key for this message, new for each message, e.g. \"ask-api-1\"",
    ))?;
    let text = PeerMessageText::new(input.text).map_err(invalid_value(&SEND, "text", "is the message body"))?;
    let reply = input
        .reply_to_message_id
        .map(MessageId::new)
        .transpose()
        .map_err(invalid_value(
            &SEND,
            "reply_to_message_id",
            "is the message_id of a delivered peer message; omit it otherwise",
        ))?;
    let response = match backend
        .send(v1::SendRequest {
            attachment_ref: wire_attachment(attachment).into(),
            recipient_peer_id: Some(recipient.as_str().to_owned()),
            idempotency_key: Some(key.as_str().to_owned()),
            text: Some(text.as_str().to_owned()),
            reply_to_message_id: reply.map(|id| id.as_str().to_owned()),
        })
        .await
    {
        Ok(response) => response,
        Err(error) => return Err(send_failure(backend, attachment, directory, &recipient, Box::new(error)).await),
    };
    let metadata = response.message.as_option().ok_or(PeerMcpError::InvalidMetadata)?;
    if metadata.recipient_peer_id.as_deref() != Some(recipient.as_str())
        || metadata.content_size_bytes.map(|size| size as usize) != Some(text.as_str().len())
    {
        return Err(PeerMcpError::InvalidMetadata);
    }
    metadata_result(message_metadata(Some(metadata), attachment, config)?)
}

/// Names the cause when a failed send went to a peer the whole roster does not list.
async fn send_failure<B: ReadPeerRoster>(
    backend: &B,
    attachment: &AttachmentRef,
    directory: &Mutex<PeerDirectory>,
    recipient: &PeerId,
    source: Box<dyn std::error::Error + Send + Sync>,
) -> PeerMcpError {
    if directory.lock().await.name(recipient.as_str()).is_some() {
        return PeerMcpError::Send(source);
    }
    let refreshed = tokio::time::timeout(DIRECTORY_BUDGET, refresh_directory(backend, attachment, directory)).await;
    let directory = directory.lock().await;
    if !matches!(refreshed, Ok(Some(RosterPage::Whole))) || directory.name(recipient.as_str()).is_some() {
        return PeerMcpError::Send(source);
    }
    PeerMcpError::UnknownRecipient {
        recipient: recipient.clone(),
        recipients: directory.recipients(attachment.peer_id()),
        source,
    }
}

pub(crate) async fn status<B: ReadPeerMessageStatus>(
    backend: &B,
    request: CallToolRequestParams,
    attachment: &AttachmentRef,
    config: &ConnectorConfig,
) -> Result<CallToolResult, PeerMcpError> {
    let input: StatusInput = parse_input(request, &STATUS)?;
    let key = IdempotencyKey::new(input.idempotency_key).map_err(invalid_value(
        &STATUS,
        "idempotency_key",
        "is the idempotency_key you passed to peer_send",
    ))?;
    let response = backend
        .status(v1::StatusRequest {
            attachment_ref: wire_attachment(attachment).into(),
            idempotency_key: Some(key.as_str().to_owned()),
        })
        .await
        .map_err(|error| PeerMcpError::Status(Box::new(error)))?;
    let message = message_metadata(response.message.as_option(), attachment, config)?;
    let status = response
        .status
        .as_ref()
        .and_then(|value| value.as_known())
        .ok_or(PeerMcpError::InvalidMetadata)?;
    if status == v1::MessageStatus::MESSAGE_STATUS_UNSPECIFIED {
        return Err(PeerMcpError::InvalidMetadata);
    }
    let output = StatusMetadataProjection {
        message,
        status,
        observed_at: timestamp_metadata(response.observed_at.as_option())?,
    };
    metadata_result(output)
}

/// `pending` is the batch this connector last delivered and has not seen acknowledged.
pub(crate) async fn acknowledge<B: AcknowledgePeerBatch>(
    backend: &B,
    request: CallToolRequestParams,
    attachment: &AttachmentRef,
    pending: &Mutex<Option<BatchId>>,
) -> Result<CallToolResult, PeerMcpError> {
    let input: AcknowledgeInput = parse_input(request, &ACKNOWLEDGE)?;
    let batch_id = BatchId::new(input.batch_id).map_err(invalid_value(
        &ACKNOWLEDGE,
        "batch_id",
        "is the batch_id of the delivered peer messages",
    ))?;
    let response = match backend
        .acknowledge_batch(v1::AcknowledgeBatchRequest {
            attachment_ref: wire_attachment(attachment).into(),
            batch_id: Some(batch_id.as_str().to_owned()),
        })
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let source = Box::new(error);
            return Err(match pending.lock().await.clone() {
                Some(delivered) if delivered == batch_id => PeerMcpError::Acknowledge(source),
                waiting => PeerMcpError::UnknownBatch {
                    requested: batch_id,
                    waiting,
                    source,
                },
            });
        }
    };
    let output = acknowledgment_metadata(&response, &batch_id)?;
    let mut delivered = pending.lock().await;
    if delivered.as_ref() == Some(&batch_id) {
        *delivered = None;
    }
    metadata_result(output)
}

fn invalid_value(
    spec: &'static ModelToolSpec,
    field: &'static str,
    rule: &'static str,
) -> impl FnOnce(PeerValueError) -> PeerMcpError {
    move |source| {
        PeerMcpError::InvalidModelInput(ToolArgumentError::new(
            spec,
            ArgumentIssue::InvalidValue { field, rule, source },
        ))
    }
}

pub(crate) fn wire_attachment(attachment: &AttachmentRef) -> v1::AttachmentRef {
    v1::AttachmentRef {
        peer_id: Some(attachment.peer_id().as_str().to_owned()),
        connector_id: Some(attachment.connector_id().as_str().to_owned()),
        generation: Some(attachment.generation()),
    }
}

fn parse_input<T: DeserializeOwned>(
    request: CallToolRequestParams,
    spec: &'static ModelToolSpec,
) -> Result<T, PeerMcpError> {
    let arguments = request.arguments.unwrap_or_default();
    spec.check(&arguments)
        .map_err(|issue| PeerMcpError::InvalidToolArguments(ToolArgumentError::new(spec, issue)))?;
    serde_json::from_value(Value::Object(arguments)).map_err(PeerMcpError::InvalidArguments)
}

fn metadata_result<T: Serialize>(projection: T) -> Result<CallToolResult, PeerMcpError> {
    let value = serde_json::to_value(projection).map_err(PeerMcpError::MetadataEncoding)?;
    let text = serde_json::to_string(&value).map_err(PeerMcpError::Encode)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}
