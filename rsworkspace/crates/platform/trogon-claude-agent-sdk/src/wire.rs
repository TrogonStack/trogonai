#[path = "runtime_payload.rs"]
pub(crate) mod runtime_payload;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ControlRequestId, DenialDisposition, JsonObject, Message, MessageDecoding, PermissionDecision, ProtocolError,
    ToolName, ToolPermissionRequest, ToolUseId, message_decode::Decoded,
};

pub(crate) enum Frame {
    Message(Message),
    /// TypeScript consumes `keep_alive` before anything else sees it.
    KeepAlive,
    /// Python `parse_message` skipped the message: the run counts it, the caller never sees it.
    Unparsed,
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
    Permission(Box<ToolPermissionRequest>),
    Hook {
        callback_id: String,
        input: JsonObject,
        tool_use_id: Option<ToolUseId>,
    },
    /// The `server_name` and `message` fields as sent: either may be missing or of any JSON type, which the
    /// TypeScript and Python routing each read their own way.
    Mcp {
        server_name: Option<Value>,
        message: Option<Value>,
    },
    Elicitation(ElicitationWire),
    UserDialog {
        kind: String,
        payload: JsonObject,
        tool_use_id: Option<ToolUseId>,
    },
    OAuthTokenRefresh {
        kind: Option<Value>,
        token_state: Option<String>,
    },
    HostAuthTokenRefresh {
        status: Option<Value>,
    },
    Unsupported {
        subtype: String,
    },
    Malformed(ProtocolError),
}

pub(crate) enum ControlResponse {
    /// The `response` field, `None` when the success frame has none.
    Success(Option<Value>),
    Error {
        message: String,
        error_code: Option<Value>,
    },
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
struct OAuthTokenWire {
    kind: Option<Value>,
    /// Read as TypeScript compares it, so a non-string state is just not one of the known ones.
    token_state: Option<Value>,
}

#[derive(Deserialize)]
struct HostAuthTokenWire {
    status: Option<Value>,
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
        #[serde(default, deserialize_with = "present")]
        response: Option<Value>,
    },
    #[serde(rename = "error")]
    Error {
        request_id: String,
        error: String,
        #[serde(default)]
        error_code: Option<Value>,
    },
}

#[derive(Deserialize)]
struct CancelWire {
    request_id: String,
}

/// Keeps a `null` field apart from a missing one: a present field is `Some`, even when it is `null`.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

fn decode<T: serde::de::DeserializeOwned>(data: JsonObject) -> Result<T, ProtocolError> {
    Ok(serde_json::from_value(Value::Object(data))?)
}

fn optional_tool_id(value: Option<String>) -> Result<Option<ToolUseId>, ProtocolError> {
    Ok(value.map(ToolUseId::from))
}

/// Takes `key` out of `fields` when `accept` reads it, leaving any other value where it was.
fn take<T>(fields: &mut JsonObject, key: &str, accept: impl FnOnce(&Value) -> Option<T>) -> Option<T> {
    let value = fields.get(key).and_then(accept)?;
    fields.shift_remove(key);
    Some(value)
}

fn string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// TypeScript `computerFolder`: an object with a string `path` and `computer_name`, or nothing.
fn computer_folder(value: &Value) -> Option<crate::ComputerFolder> {
    Some(crate::ComputerFolder {
        path: string(value.get("path")?)?,
        computer_name: string(value.get("computer_name")?)?,
    })
}

/// TypeScript `matchedAskRule`, with `ruleContent` only when the wire carries one.
fn matched_ask_rule(value: &Value) -> Option<crate::MatchedAskRule> {
    let rule_content = match value.get("rule_content") {
        None => None,
        Some(content) => Some(string(content)?),
    };
    Some(crate::MatchedAskRule {
        source: string(value.get("source")?)?,
        tool_name: string(value.get("tool_name")?)?,
        rule_content,
    })
}

fn permission_request(data: JsonObject, id: &ControlRequestId) -> Result<ToolPermissionRequest, ProtocolError> {
    let mut wire: PermissionRequestWire = decode(data)?;
    let fields = &mut wire.additional_fields;
    let decision_reason_type = take(fields, "decision_reason_type", string);
    let decision_reason_code = take(fields, "decision_reason_code", string);
    let server_policy_ask_only = take(fields, "server_policy_ask_only", Value::as_bool);
    let plugin = take(fields, "plugin", string);
    let server_prompt = take(fields, "server_prompt", string);
    let computer_folder = take(fields, "computer_folder", computer_folder);
    let matched_ask_rule = take(fields, "matched_ask_rule", matched_ask_rule);
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
        decision_reason_type,
        decision_reason_code,
        server_policy_ask_only: server_policy_ask_only.into(),
        plugin,
        server_prompt,
        computer_folder,
        matched_ask_rule,
        additional_fields: wire.additional_fields,
    })
}

#[cfg(test)]
pub(crate) fn parse_frame(bytes: &[u8]) -> Result<Frame, ProtocolError> {
    parse_value(&serde_json::from_slice(bytes)?, MessageDecoding::TypeScript)
}

/// Decodes an object a [`crate::Transport`] already parsed as JSON.
pub(crate) fn parse_value(object: &JsonObject, mode: MessageDecoding) -> Result<Frame, ProtocolError> {
    let data = || object.clone();
    Ok(match object.get("type").and_then(Value::as_str) {
        Some("control_request") => {
            let wire: IncomingControlWire = decode(data())?;
            let id = ControlRequestId::from(wire.request_id);
            let request = if wire.request.subtype == "can_use_tool" {
                match permission_request(wire.request.data, &id) {
                    Ok(request) => IncomingControl::Permission(Box::new(request)),
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
                IncomingControl::Mcp {
                    server_name: wire.request.data.get("server_name").cloned(),
                    message: wire.request.data.get("message").cloned(),
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
            } else if wire.request.subtype == "oauth_token_refresh" {
                let wire: OAuthTokenWire = decode(wire.request.data)?;
                IncomingControl::OAuthTokenRefresh {
                    kind: wire.kind,
                    token_state: wire.token_state.as_ref().and_then(Value::as_str).map(str::to_owned),
                }
            } else if wire.request.subtype == "host_auth_token_refresh" {
                let wire: HostAuthTokenWire = decode(wire.request.data)?;
                IncomingControl::HostAuthTokenRefresh { status: wire.status }
            } else {
                IncomingControl::Unsupported {
                    subtype: wire.request.subtype,
                }
            };
            Frame::ControlRequest { id, request }
        }
        Some("control_response") => {
            let wire: ControlResponseFrameWire = decode(data())?;
            match wire.response {
                ControlResponseWire::Success { request_id, response } => Frame::ControlResponse {
                    id: ControlRequestId::from(request_id),
                    response: ControlResponse::Success(response),
                },
                ControlResponseWire::Error {
                    request_id,
                    error,
                    error_code,
                } => Frame::ControlResponse {
                    id: ControlRequestId::from(request_id),
                    response: ControlResponse::Error {
                        message: error,
                        error_code,
                    },
                },
            }
        }
        Some("control_cancel_request") => {
            let wire: CancelWire = decode(data())?;
            Frame::Cancel {
                id: ControlRequestId::from(wire.request_id),
            }
        }
        Some("transcript_mirror") => {
            let wire: TranscriptMirrorWire = decode(data())?;
            Frame::TranscriptMirror {
                file_path: wire.file_path,
                entries: wire.entries.into_iter().map(crate::SessionEntry::new).collect(),
            }
        }
        _ => match crate::message_decode::decode(object, mode)? {
            Decoded::Message(message) => Frame::Message(*message),
            Decoded::KeepAlive => Frame::KeepAlive,
            Decoded::Unparsed => Frame::Unparsed,
        },
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

/// The `can_use_tool` answer in the dialect's shape, or `None` when the host answered out of band.
///
/// TypeScript writes `{...result, toolUseID}` with `updatedInput` only when the handler set it. Python sends the
/// original input when the handler left it unset and never adds `toolUseID`.
pub(crate) fn permission_response(
    id: &ControlRequestId,
    input: JsonObject,
    tool_use_id: Option<&ToolUseId>,
    decision: PermissionDecision,
    style: crate::ArgvStyle,
) -> Result<Option<Value>, ProtocolError> {
    let typescript = style == crate::ArgvStyle::TypeScript;
    let tool_use_id = tool_use_id.filter(|_| typescript).map(ToolUseId::as_str);
    let response = match decision {
        PermissionDecision::Allow {
            updated_input,
            updated_permissions,
            decision_classification,
        } => runtime_payload::PermissionResponsePayload::Allow {
            updated_input: if typescript {
                updated_input
            } else {
                Some(updated_input.unwrap_or(input))
            },
            updated_permissions: updated_permissions
                .map(|updates| {
                    updates
                        .iter()
                        .map(|update| {
                            if typescript {
                                update.to_wire_json()
                            } else {
                                update.to_python_wire_json()
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            decision_classification: decision_classification.map(crate::PermissionDecisionClassification::as_str),
            tool_use_id,
        },
        PermissionDecision::Deny {
            message,
            disposition,
            decision_classification,
        } => runtime_payload::PermissionResponsePayload::Deny {
            message,
            interrupt: (disposition == DenialDisposition::Interrupt).then_some(true),
            decision_classification: decision_classification.map(crate::PermissionDecisionClassification::as_str),
            tool_use_id,
        },
        PermissionDecision::AnsweredOutOfBand if typescript => return Ok(None),
        PermissionDecision::AnsweredOutOfBand => {
            return callback_error_response(
                id,
                "Tool permission callback must return PermissionResult (PermissionResultAllow or PermissionResultDeny), got <class 'NoneType'>",
            )
            .map(Some);
        }
        PermissionDecision::Failed { message } => return callback_error_response(id, &message).map(Some),
    };
    success_response(id, response).map(Some)
}

/// The answer to a `can_use_tool` request when no permission handler is registered.
pub(crate) fn missing_permission_handler_response(
    id: &ControlRequestId,
    style: crate::ArgvStyle,
) -> Result<Value, ProtocolError> {
    callback_error_response(
        id,
        match style {
            crate::ArgvStyle::TypeScript => "canUseTool callback is not provided.",
            crate::ArgvStyle::Python => "canUseTool callback is not provided",
        },
    )
}

/// The `hook_callback` answer for a callback id the initialize request did not register.
pub(crate) fn unknown_hook_response(id: &ControlRequestId, callback_id: &str) -> Result<Value, ProtocolError> {
    callback_error_response(id, &format!("No hook callback found for ID: {callback_id}"))
}

/// The answer to a failed handler. A cancelled TypeScript handler throws its signal's `AbortError`; a cancelled
/// Python handler raises the cancellation, which writes nothing.
pub(crate) fn handler_error_response(
    id: &ControlRequestId,
    error: &crate::CallbackError,
    style: crate::ArgvStyle,
) -> Result<Option<Value>, ProtocolError> {
    match (error, style) {
        (crate::CallbackError::Cancelled, crate::ArgvStyle::Python) => Ok(None),
        (crate::CallbackError::Cancelled, crate::ArgvStyle::TypeScript) => {
            callback_error_response(id, "This operation was aborted").map(Some)
        }
        (crate::CallbackError::Failed(error), _) => callback_error_response(id, &error.to_string()).map(Some),
    }
}

/// The answer to a subtype this SDK does not handle, or `None` when the dialect leaves it unanswered.
///
/// TypeScript throws its `unsupported_request` error, except for the remote tool subtypes it ignores and the auth
/// refresh subtypes whose callback is missing. Python raises one plain error for every subtype it does not
/// handle.
pub(crate) fn unsupported_response(
    id: &ControlRequestId,
    subtype: &str,
    style: crate::ArgvStyle,
) -> Option<Result<Value, ProtocolError>> {
    let message = format!("Unsupported control request subtype: {subtype}");
    match style {
        crate::ArgvStyle::Python => Some(callback_error_response(id, &message)),
        crate::ArgvStyle::TypeScript => match subtype {
            _ if crate::constants::REMOTE_TOOL_SUBTYPES.contains(&subtype) => None,
            "oauth_token_refresh" => Some(callback_error_response(id, crate::constants::OAUTH_CALLBACK_MISSING)),
            "host_auth_token_refresh" => Some(callback_error_response(
                id,
                crate::constants::HOST_AUTH_CALLBACK_MISSING,
            )),
            "remote_control_work_secret" => Some(callback_error_response(
                id,
                "refreshWorkSecret callback is not provided.",
            )),
            _ => Some(error_frame(id, &message, Some("unsupported_request"))),
        },
    }
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

fn error_frame(id: &ControlRequestId, message: &str, error_code: Option<&str>) -> Result<Value, ProtocolError> {
    let frame: runtime_payload::OutboundFrame<'_, Value> = runtime_payload::OutboundFrame::Response {
        response: runtime_payload::CallbackResponse::Error {
            request_id: id.as_str(),
            error: message,
            error_code,
        },
    };
    serde_json::to_value(frame).map_err(ProtocolError::from)
}

pub(crate) fn callback_error_response(id: &ControlRequestId, message: &str) -> Result<Value, ProtocolError> {
    error_frame(id, message, None)
}

pub(crate) fn malformed_response(id: &ControlRequestId) -> Result<Value, ProtocolError> {
    callback_error_response(id, "Malformed SDK permission request")
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
