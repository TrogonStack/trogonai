#[path = "runtime_payload.rs"]
pub(crate) mod runtime_payload;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AssistantMessage, Content, ContentBlock, ControlRequestId, DenialDisposition, JsonObject, Message, MessageId,
    MessageOrigin, PermissionDecision, ProtocolError, ResultMessage, ResultStatus, SessionId, StreamEvent,
    SystemMessage, ToolName, ToolPermissionRequest, ToolResultStatus, ToolUseId, UserMessage,
};

pub(crate) enum Frame {
    Message(Message),
    ControlRequest {
        id: ControlRequestId,
        request: IncomingControl,
    },
    ControlResponse {
        id: ControlRequestId,
        response: ControlResponse,
    },
    Cancel {
        id: ControlRequestId,
    },
    TranscriptMirror {
        file_path: std::path::PathBuf,
        entries: Vec<crate::SessionEntry>,
    },
}

pub(crate) enum IncomingControl {
    Permission(ToolPermissionRequest),
    Hook {
        callback_id: String,
        input: JsonObject,
        tool_use_id: Option<ToolUseId>,
    },
    Mcp {
        server_name: String,
        message: JsonObject,
    },
    Elicitation(ElicitationWire),
    UserDialog {
        kind: String,
        payload: JsonObject,
        tool_use_id: Option<ToolUseId>,
    },
    Unsupported,
    Malformed(ProtocolError),
}

pub(crate) enum ControlResponse {
    Success(Value),
    Error(String),
}

#[derive(Deserialize)]
struct EnvelopeWire {
    #[serde(rename = "type")]
    kind: String,
    #[serde(flatten)]
    data: JsonObject,
}

#[derive(Deserialize)]
struct AssistantWire {
    message: AssistantBodyWire,
    parent_tool_use_id: Option<String>,
    session_id: Option<String>,
    error: Option<String>,
    uuid: Option<String>,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Deserialize)]
struct AssistantBodyWire {
    content: Vec<Value>,
    model: String,
    usage: Option<JsonObject>,
    id: Option<String>,
    stop_reason: Option<String>,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Deserialize)]
struct UserWire {
    message: UserBodyWire,
    parent_tool_use_id: Option<String>,
    session_id: Option<String>,
    uuid: Option<String>,
    origin: Option<MessageOrigin>,
    #[serde(default, deserialize_with = "deserialize_present_json")]
    tool_use_result: Option<Value>,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Deserialize)]
struct UserBodyWire {
    content: ContentWire,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ContentWire {
    Text(String),
    Blocks(Vec<Value>),
}

#[derive(Deserialize)]
struct SystemWire {
    subtype: String,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct ResultWire {
    subtype: String,
    is_error: bool,
    duration_ms: u64,
    duration_api_ms: u64,
    num_turns: u64,
    session_id: String,
    total_cost_usd: Option<f64>,
    result: Option<String>,
    structured_output: Option<Value>,
    usage: Option<JsonObject>,
    errors: Option<Vec<String>>,
    uuid: Option<String>,
    stop_reason: Option<String>,
    terminal_reason: Option<String>,
    origin: Option<MessageOrigin>,
    #[serde(rename = "modelUsage")]
    model_usage: Option<JsonObject>,
    permission_denials: Option<Vec<Value>>,
    deferred_tool_use: Option<JsonObject>,
    api_error_status: Option<u16>,
    result_index: Option<u64>,
    queued_turn_count: Option<u64>,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Deserialize)]
struct StreamEventWire {
    uuid: String,
    session_id: String,
    event: JsonObject,
    parent_tool_use_id: Option<String>,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Deserialize)]
struct TextWire {
    text: String,
}

#[derive(Deserialize)]
struct ThinkingWire {
    thinking: String,
    signature: String,
}

#[derive(Deserialize)]
struct ToolUseWire {
    id: String,
    name: String,
    input: JsonObject,
}

#[derive(Deserialize)]
struct ToolResultWire {
    tool_use_id: String,
    #[serde(default)]
    content: Value,
    is_error: Option<bool>,
}

#[derive(Deserialize)]
struct IncomingControlWire {
    request_id: String,
    request: IncomingRequestWire,
}

#[derive(Deserialize)]
struct IncomingRequestWire {
    subtype: String,
    #[serde(flatten)]
    data: JsonObject,
}

#[derive(Deserialize)]
struct PermissionRequestWire {
    tool_name: String,
    input: JsonObject,
    tool_use_id: Option<String>,
    blocked_path: Option<String>,
    decision_reason: Option<String>,
    permission_suggestions: Option<Vec<JsonObject>>,
    agent_id: Option<String>,
    mcp_server: Option<McpProvenanceWire>,
    title: Option<String>,
    display_name: Option<String>,
    description: Option<String>,
    default_to_no: Option<bool>,
    suppress_always_allow_rule: Option<bool>,
    requires_user_interaction: Option<bool>,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Deserialize)]
struct McpProvenanceWire {
    name: String,
    source: String,
}

#[derive(Deserialize)]
struct HookRequestWire {
    callback_id: String,
    input: JsonObject,
    tool_use_id: Option<String>,
}

#[derive(Deserialize)]
struct McpRequestWire {
    server_name: String,
    message: JsonObject,
}

#[derive(Deserialize)]
struct UserDialogWire {
    dialog_kind: String,
    payload: JsonObject,
    tool_use_id: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ElicitationWire {
    #[serde(rename = "mcp_server_name")]
    pub server_name: String,
    pub message: String,
    pub mode: Option<String>,
    pub url: Option<String>,
    pub elicitation_id: Option<String>,
    pub requested_schema: Option<JsonObject>,
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

#[derive(Deserialize)]
struct TranscriptMirrorWire {
    #[serde(rename = "filePath")]
    file_path: std::path::PathBuf,
    entries: Vec<JsonObject>,
}

#[derive(Deserialize)]
struct ControlResponseFrameWire {
    response: ControlResponseWire,
}

#[derive(Deserialize)]
#[serde(tag = "subtype")]
enum ControlResponseWire {
    #[serde(rename = "success")]
    Success {
        request_id: String,
        #[serde(default)]
        response: Value,
    },
    #[serde(rename = "error")]
    Error { request_id: String, error: String },
}

#[derive(Deserialize)]
struct CancelWire {
    request_id: String,
}

fn deserialize_present_json<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

fn decode<T: serde::de::DeserializeOwned>(data: JsonObject) -> Result<T, ProtocolError> {
    Ok(serde_json::from_value(Value::Object(data))?)
}

fn optional_tool_id(value: Option<String>) -> Result<Option<ToolUseId>, ProtocolError> {
    Ok(value.map(ToolUseId::from))
}

fn optional_session(value: Option<String>) -> Result<Option<SessionId>, ProtocolError> {
    Ok(value.map(SessionId::from))
}

fn content_blocks(values: Vec<Value>) -> Result<Vec<ContentBlock>, ProtocolError> {
    values
        .into_iter()
        .map(|value| {
            let envelope: EnvelopeWire = serde_json::from_value(value)?;
            Ok(match envelope.kind.as_str() {
                "text" => {
                    let wire: TextWire = decode(envelope.data)?;
                    ContentBlock::Text { text: wire.text }
                }
                "thinking" => {
                    let wire: ThinkingWire = decode(envelope.data)?;
                    ContentBlock::Thinking {
                        thinking: wire.thinking,
                        signature: wire.signature,
                    }
                }
                "tool_use" => {
                    let wire: ToolUseWire = decode(envelope.data)?;
                    ContentBlock::ToolUse {
                        id: ToolUseId::from(wire.id),
                        name: ToolName::from(wire.name),
                        input: wire.input,
                    }
                }
                "tool_result" => {
                    let wire: ToolResultWire = decode(envelope.data)?;
                    ContentBlock::ToolResult {
                        tool_use_id: ToolUseId::from(wire.tool_use_id),
                        content: wire.content,
                        status: match wire.is_error {
                            Some(true) => ToolResultStatus::Error,
                            Some(false) => ToolResultStatus::Success,
                            None => ToolResultStatus::NotReported,
                        },
                    }
                }
                _ => ContentBlock::Unknown {
                    kind: envelope.kind,
                    data: envelope.data,
                },
            })
        })
        .collect()
}

fn permission_request(data: JsonObject, id: &ControlRequestId) -> Result<ToolPermissionRequest, ProtocolError> {
    let wire: PermissionRequestWire = decode(data)?;
    Ok(ToolPermissionRequest {
        tool_name: ToolName::from(wire.tool_name),
        input: wire.input,
        tool_use_id: optional_tool_id(wire.tool_use_id)?,
        blocked_path: wire.blocked_path,
        decision_reason: wire.decision_reason,
        request_id: id.clone(),
        signal: crate::CancellationToken::new(),
        suggestions: wire
            .permission_suggestions
            .unwrap_or_default()
            .into_iter()
            .map(crate::PermissionUpdate::from_wire_json)
            .collect::<Result<_, _>>()?,
        agent_id: wire.agent_id,
        mcp_server: wire.mcp_server.map(|value| crate::McpServerProvenance {
            name: value.name,
            source: value.source,
        }),
        title: wire.title,
        display_name: wire.display_name,
        description: wire.description,
        default_to_no: wire.default_to_no.into(),
        suppress_always_allow_rule: wire.suppress_always_allow_rule.into(),
        requires_user_interaction: wire.requires_user_interaction.into(),
        additional_fields: wire.additional_fields,
    })
}

#[cfg(test)]
pub(crate) fn parse_frame(bytes: &[u8]) -> Result<Frame, ProtocolError> {
    frame_from_envelope(serde_json::from_slice(bytes)?)
}

/// Decodes a message a [`crate::Transport`] already parsed as JSON.
pub(crate) fn parse_value(value: &serde_json::Value) -> Result<Frame, ProtocolError> {
    frame_from_envelope(EnvelopeWire::deserialize(value)?)
}

fn frame_from_envelope(envelope: EnvelopeWire) -> Result<Frame, ProtocolError> {
    Ok(match envelope.kind.as_str() {
        "assistant" => {
            let wire: AssistantWire = decode(envelope.data)?;
            Frame::Message(Message::Assistant(AssistantMessage {
                content: content_blocks(wire.message.content)?,
                model: wire.message.model,
                parent_tool_use_id: optional_tool_id(wire.parent_tool_use_id)?,
                session_id: optional_session(wire.session_id)?,
                error: wire.error,
                usage: wire.message.usage,
                uuid: wire.uuid.map(MessageId::from),
                message_id: wire.message.id.map(MessageId::from),
                stop_reason: wire.message.stop_reason,
                additional_fields: wire.additional_fields,
                message_fields: wire.message.additional_fields,
            }))
        }
        "user" => {
            let wire: UserWire = decode(envelope.data)?;
            Frame::Message(Message::User(UserMessage {
                content: match wire.message.content {
                    ContentWire::Text(text) => Content::Text(text),
                    ContentWire::Blocks(blocks) => Content::Blocks(content_blocks(blocks)?),
                },
                parent_tool_use_id: optional_tool_id(wire.parent_tool_use_id)?,
                session_id: optional_session(wire.session_id)?,
                uuid: wire.uuid.map(MessageId::from),
                origin: wire.origin,
                tool_use_result: wire.tool_use_result,
                additional_fields: wire.additional_fields,
                message_fields: wire.message.additional_fields,
            }))
        }
        "system" => {
            let wire: SystemWire = decode(envelope.data.clone())?;
            Frame::Message(Message::System(SystemMessage {
                subtype: wire.subtype,
                session_id: optional_session(wire.session_id)?,
                data: envelope.data,
            }))
        }
        "result" => {
            let wire: ResultWire = decode(envelope.data)?;
            Frame::Message(Message::Result(Box::new(ResultMessage {
                subtype: wire.subtype,
                status: if wire.is_error {
                    ResultStatus::Error
                } else {
                    ResultStatus::Success
                },
                duration_ms: wire.duration_ms,
                duration_api_ms: wire.duration_api_ms,
                num_turns: wire.num_turns,
                session_id: SessionId::from(wire.session_id),
                total_cost_usd: wire.total_cost_usd,
                result: wire.result,
                structured_output: wire.structured_output,
                usage: wire.usage,
                errors: wire.errors,
                uuid: wire.uuid.map(MessageId::from),
                stop_reason: wire.stop_reason,
                terminal_reason: wire.terminal_reason,
                origin: wire.origin,
                model_usage: wire.model_usage,
                permission_denials: wire.permission_denials,
                deferred_tool_use: wire.deferred_tool_use,
                api_error_status: wire.api_error_status,
                result_index: wire.result_index,
                queued_turn_count: wire.queued_turn_count,
                additional_fields: wire.additional_fields,
            })))
        }
        "stream_event" => {
            let wire: StreamEventWire = decode(envelope.data)?;
            Frame::Message(Message::StreamEvent(StreamEvent {
                uuid: wire.uuid,
                session_id: SessionId::from(wire.session_id),
                event: wire.event,
                additional_fields: wire.additional_fields,
                parent_tool_use_id: optional_tool_id(wire.parent_tool_use_id)?,
            }))
        }
        "control_request" => {
            let wire: IncomingControlWire = decode(envelope.data)?;
            let id = ControlRequestId::from(wire.request_id);
            let request = if wire.request.subtype == "can_use_tool" {
                match permission_request(wire.request.data, &id) {
                    Ok(request) => IncomingControl::Permission(request),
                    Err(error) => IncomingControl::Malformed(error),
                }
            } else if wire.request.subtype == "hook_callback" {
                let wire: HookRequestWire = decode(wire.request.data)?;
                IncomingControl::Hook {
                    callback_id: wire.callback_id,
                    input: wire.input,
                    tool_use_id: optional_tool_id(wire.tool_use_id)?,
                }
            } else if wire.request.subtype == "mcp_message" {
                let wire: McpRequestWire = decode(wire.request.data)?;
                IncomingControl::Mcp {
                    server_name: wire.server_name,
                    message: wire.message,
                }
            } else if wire.request.subtype == "elicitation" {
                IncomingControl::Elicitation(decode(wire.request.data)?)
            } else if wire.request.subtype == "request_user_dialog" {
                let wire: UserDialogWire = decode(wire.request.data)?;
                IncomingControl::UserDialog {
                    kind: wire.dialog_kind,
                    payload: wire.payload,
                    tool_use_id: optional_tool_id(wire.tool_use_id)?,
                }
            } else {
                IncomingControl::Unsupported
            };
            Frame::ControlRequest { id, request }
        }
        "control_response" => {
            let wire: ControlResponseFrameWire = decode(envelope.data)?;
            match wire.response {
                ControlResponseWire::Success { request_id, response } => Frame::ControlResponse {
                    id: ControlRequestId::from(request_id),
                    response: ControlResponse::Success(response),
                },
                ControlResponseWire::Error { request_id, error } => Frame::ControlResponse {
                    id: ControlRequestId::from(request_id),
                    response: ControlResponse::Error(error),
                },
            }
        }
        "control_cancel_request" => {
            let wire: CancelWire = decode(envelope.data)?;
            Frame::Cancel {
                id: ControlRequestId::from(wire.request_id),
            }
        }
        "transcript_mirror" => {
            let wire: TranscriptMirrorWire = decode(envelope.data)?;
            Frame::TranscriptMirror {
                file_path: wire.file_path,
                entries: wire.entries.into_iter().map(crate::SessionEntry::new).collect(),
            }
        }
        _ => Frame::Message(Message::Unknown {
            kind: envelope.kind,
            data: envelope.data,
        }),
    })
}

pub(crate) fn control_request(
    id: &ControlRequestId,
    request: Value,
    style: crate::ArgvStyle,
) -> Result<Value, ProtocolError> {
    match style {
        crate::ArgvStyle::TypeScript => serde_json::to_value(runtime_payload::TypeScriptControlRequest {
            request_id: id.as_str(),
            kind: "control_request",
            request,
        }),
        crate::ArgvStyle::Python => serde_json::to_value(runtime_payload::OutboundFrame::Request {
            request_id: id.as_str(),
            request,
        }),
    }
    .map_err(ProtocolError::from)
}

pub(crate) fn permission_response(
    id: &ControlRequestId,
    input: JsonObject,
    decision: PermissionDecision,
) -> Result<Value, ProtocolError> {
    let response = match decision {
        PermissionDecision::Allow {
            updated_input,
            updated_permissions,
        } => runtime_payload::PermissionResponsePayload::Allow {
            updated_input: updated_input.unwrap_or(input),
            updated_permissions: updated_permissions
                .map(|updates| {
                    updates
                        .iter()
                        .map(crate::PermissionUpdate::to_wire_json)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
        },
        PermissionDecision::Deny { message, disposition } => runtime_payload::PermissionResponsePayload::Deny {
            message,
            interrupt: disposition == DenialDisposition::Interrupt,
        },
    };
    success_response(id, response)
}

pub(crate) fn success_response<T: Serialize>(id: &ControlRequestId, response: T) -> Result<Value, ProtocolError> {
    serde_json::to_value(runtime_payload::OutboundFrame::Response {
        response: runtime_payload::CallbackResponse::Success {
            request_id: id.as_str(),
            response,
        },
    })
    .map_err(ProtocolError::from)
}

pub(crate) fn callback_error_response(id: &ControlRequestId, message: &'static str) -> Result<Value, ProtocolError> {
    let frame: runtime_payload::OutboundFrame<'_, Value> = runtime_payload::OutboundFrame::Response {
        response: runtime_payload::CallbackResponse::Error {
            request_id: id.as_str(),
            error: message,
        },
    };
    serde_json::to_value(frame).map_err(ProtocolError::from)
}

pub(crate) fn malformed_response(id: &ControlRequestId) -> Result<Value, ProtocolError> {
    callback_error_response(id, "Malformed SDK permission request")
}

pub(crate) fn unsupported_response(id: &ControlRequestId) -> Result<Value, ProtocolError> {
    callback_error_response(id, "Unsupported SDK control request")
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
