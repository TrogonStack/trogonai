//! Decoding of the messages the CLI writes, as each upstream SDK does it (see [`MessageDecoding`]).

use serde::Deserialize;
use serde_json::Value;

use crate::{
    AssistantMessage, Content, ContentBlock, ConversationResetMessage, HookEventMessage, JsonObject, Message,
    MessageDecoding, MessageId, MessageOrigin, MirrorErrorMessage, ProtocolError, RateLimitEvent, RateLimitInfo,
    ResultMessage, ResultStatus, SessionId, StreamEvent, SystemEvent, SystemMessage, TaskNotificationMessage,
    TaskProgressMessage, TaskStartedMessage, TaskUpdatedMessage, ToolName, ToolResultStatus, ToolUseId, UserMessage,
};

#[path = "message_decode/python.rs"]
mod python;

/// What one message line becomes.
pub(crate) enum Decoded {
    /// A message for the caller.
    Message(Message),
    /// TypeScript consumes `keep_alive` before anything else sees it.
    KeepAlive,
    /// Python `parse_message` returned `None`: the run still counts the message, the caller never sees it.
    Unparsed,
}

/// Decodes one message object that is not a control, cancel or transcript mirror frame.
pub(crate) fn decode(raw: &JsonObject, mode: MessageDecoding) -> Result<Decoded, ProtocolError> {
    let kind = raw.get("type").and_then(Value::as_str);
    match mode {
        MessageDecoding::TypeScript => {
            if kind == Some("keep_alive") {
                return Ok(Decoded::KeepAlive);
            }
        }
        MessageDecoding::Python => {
            python::check(raw)?;
            if !matches!(
                kind,
                Some(
                    "user"
                        | "assistant"
                        | "system"
                        | "result"
                        | "stream_event"
                        | "rate_limit_event"
                        | "conversation_reset"
                )
            ) {
                return Ok(Decoded::Unparsed);
            }
        }
    }
    Ok(Decoded::Message(typed(raw, mode)))
}

/// The message as its Rust struct, or [`Message::Unknown`] when its type has none or its values do not fit it.
pub(crate) fn typed(raw: &JsonObject, mode: MessageDecoding) -> Message {
    let built = match raw.get("type").and_then(Value::as_str) {
        Some("assistant") => assistant(raw, mode).map(Message::Assistant),
        Some("user") => user(raw, mode).map(Message::User),
        Some("system") => system(raw).map(Message::System),
        Some("result") => result(raw).map(|message| Message::Result(Box::new(message))),
        Some("stream_event") => stream_event(raw).map(Message::StreamEvent),
        Some("rate_limit_event") => rate_limit(raw).map(Message::RateLimit),
        Some("conversation_reset") => conversation_reset(raw).map(Message::ConversationReset),
        _ => None,
    };
    built.unwrap_or_else(|| {
        let (kind, data) = split_type(raw);
        Message::Unknown {
            kind,
            data,
            raw: raw.clone(),
        }
    })
}

/// The `type` (empty when absent or not a string) and the object without it.
fn split_type(object: &JsonObject) -> (String, JsonObject) {
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut data = object.clone();
    data.shift_remove("type");
    (kind, data)
}

fn fields<T: serde::de::DeserializeOwned>(object: &JsonObject) -> Option<T> {
    let (_, data) = split_type(object);
    serde_json::from_value(Value::Object(data)).ok()
}

/// Python `_parse_origin`: an object with a string `kind`, anything else is absent.
fn origin(value: Option<Value>) -> Option<MessageOrigin> {
    value.and_then(|value| serde_json::from_value(value).ok())
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
    origin: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    tool_use_result: Option<Value>,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Deserialize)]
struct UserBodyWire {
    content: Value,
    #[serde(flatten)]
    additional_fields: JsonObject,
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
    origin: Option<Value>,
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
struct RateLimitWire {
    rate_limit_info: RateLimitInfoWire,
    uuid: String,
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitInfoWire {
    status: String,
    resets_at: Option<i64>,
    rate_limit_type: Option<String>,
    utilization: Option<f64>,
    overage_status: Option<String>,
    overage_resets_at: Option<i64>,
    overage_disabled_reason: Option<String>,
}

#[derive(Deserialize)]
struct ConversationResetWire {
    new_conversation_id: String,
    uuid: String,
    session_id: String,
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
struct ServerToolResultWire {
    tool_use_id: String,
    content: Value,
}

fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// Which content list a block is in: Python parses fewer block kinds in user messages.
#[derive(Clone, Copy)]
enum Side {
    Assistant,
    User,
}

/// TypeScript keeps every block, typing the ones Rust models and keeping the rest as [`ContentBlock::Unknown`].
/// Python skips the blocks of a kind `parse_message` does not parse. A block that is not an object fails the
/// message in both modes (Python rejects it before this runs).
fn blocks(values: Vec<Value>, side: Side, mode: MessageDecoding) -> Option<Vec<ContentBlock>> {
    let mut blocks = Vec::with_capacity(values.len());
    for value in values {
        let Value::Object(object) = value else {
            return None;
        };
        let kind = object.get("type").and_then(Value::as_str).unwrap_or_default();
        if mode == MessageDecoding::Python {
            let parsed = match side {
                Side::User => matches!(kind, "text" | "tool_use" | "tool_result"),
                Side::Assistant => matches!(
                    kind,
                    "text" | "thinking" | "tool_use" | "tool_result" | "server_tool_use" | "advisor_tool_result"
                ),
            };
            if !parsed {
                continue;
            }
        }
        blocks.push(block(&object).unwrap_or_else(|| {
            let (kind, data) = split_type(&object);
            ContentBlock::Unknown { kind, data }
        }));
    }
    Some(blocks)
}

fn block(object: &JsonObject) -> Option<ContentBlock> {
    Some(match object.get("type").and_then(Value::as_str)? {
        "text" => {
            let wire: TextWire = fields(object)?;
            ContentBlock::Text { text: wire.text }
        }
        "thinking" => {
            let wire: ThinkingWire = fields(object)?;
            ContentBlock::Thinking {
                thinking: wire.thinking,
                signature: wire.signature,
            }
        }
        "tool_use" => {
            let wire: ToolUseWire = fields(object)?;
            ContentBlock::ToolUse {
                id: ToolUseId::from(wire.id),
                name: ToolName::from(wire.name),
                input: wire.input,
            }
        }
        "tool_result" => {
            let wire: ToolResultWire = fields(object)?;
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
        "server_tool_use" => {
            let wire: ToolUseWire = fields(object)?;
            ContentBlock::ServerToolUse {
                id: ToolUseId::from(wire.id),
                name: wire.name,
                input: wire.input,
            }
        }
        "advisor_tool_result" => {
            let wire: ServerToolResultWire = fields(object)?;
            ContentBlock::ServerToolResult {
                tool_use_id: ToolUseId::from(wire.tool_use_id),
                content: wire.content,
            }
        }
        _ => return None,
    })
}

fn assistant(raw: &JsonObject, mode: MessageDecoding) -> Option<AssistantMessage> {
    let wire: AssistantWire = fields(raw)?;
    Some(AssistantMessage {
        content: blocks(wire.message.content, Side::Assistant, mode)?,
        model: wire.message.model,
        parent_tool_use_id: wire.parent_tool_use_id.map(ToolUseId::from),
        session_id: wire.session_id.map(SessionId::from),
        error: wire.error,
        usage: wire.message.usage,
        uuid: wire.uuid.map(MessageId::from),
        message_id: wire.message.id.map(MessageId::from),
        stop_reason: wire.message.stop_reason,
        additional_fields: wire.additional_fields,
        message_fields: wire.message.additional_fields,
        raw: raw.clone(),
    })
}

fn user(raw: &JsonObject, mode: MessageDecoding) -> Option<UserMessage> {
    let wire: UserWire = fields(raw)?;
    Some(UserMessage {
        content: match wire.message.content {
            Value::String(text) => Content::Text(text),
            Value::Array(values) => Content::Blocks(blocks(values, Side::User, mode)?),
            _ => return None,
        },
        parent_tool_use_id: wire.parent_tool_use_id.map(ToolUseId::from),
        session_id: wire.session_id.map(SessionId::from),
        uuid: wire.uuid.map(MessageId::from),
        origin: origin(wire.origin),
        tool_use_result: wire.tool_use_result,
        additional_fields: wire.additional_fields,
        message_fields: wire.message.additional_fields,
        raw: raw.clone(),
    })
}

pub(crate) fn system(raw: &JsonObject) -> Option<SystemMessage> {
    let wire: SystemWire = fields(raw)?;
    let event = system_event(raw, &wire.subtype);
    let (_, data) = split_type(raw);
    Some(SystemMessage {
        subtype: wire.subtype,
        session_id: wire.session_id.map(SessionId::from),
        data,
        event,
        raw: raw.clone(),
    })
}

fn text(raw: &JsonObject, key: &str) -> Option<String> {
    raw.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// An optional string: absent and `null` are `None`, any other non-string fails the view.
fn optional_text(raw: &JsonObject, key: &str) -> Option<Option<String>> {
    match raw.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.clone())),
        Some(_) => None,
    }
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(values)) => !values.is_empty(),
        Some(Value::Object(object)) => !object.is_empty(),
    }
}

fn system_event(raw: &JsonObject, subtype: &str) -> SystemEvent {
    let event = match subtype {
        "hook_started" | "hook_response" => hook_event(raw).map(SystemEvent::Hook),
        "task_started" => (|| {
            Some(SystemEvent::TaskStarted(TaskStartedMessage {
                task_id: text(raw, "task_id")?,
                description: text(raw, "description")?,
                uuid: text(raw, "uuid")?,
                session_id: text(raw, "session_id")?,
                tool_use_id: optional_text(raw, "tool_use_id")?,
                task_type: optional_text(raw, "task_type")?,
            }))
        })(),
        "task_progress" => (|| {
            Some(SystemEvent::TaskProgress(TaskProgressMessage {
                task_id: text(raw, "task_id")?,
                description: text(raw, "description")?,
                usage: raw.get("usage")?.clone(),
                uuid: text(raw, "uuid")?,
                session_id: text(raw, "session_id")?,
                tool_use_id: optional_text(raw, "tool_use_id")?,
                last_tool_name: optional_text(raw, "last_tool_name")?,
            }))
        })(),
        "task_notification" => (|| {
            Some(SystemEvent::TaskNotification(TaskNotificationMessage {
                task_id: text(raw, "task_id")?,
                status: text(raw, "status")?,
                output_file: text(raw, "output_file")?,
                summary: text(raw, "summary")?,
                uuid: text(raw, "uuid")?,
                session_id: text(raw, "session_id")?,
                tool_use_id: optional_text(raw, "tool_use_id")?,
                usage: raw.get("usage").filter(|value| !value.is_null()).cloned(),
            }))
        })(),
        "task_updated" => (|| {
            let patch = raw.get("patch").and_then(Value::as_object).cloned().unwrap_or_default();
            Some(SystemEvent::TaskUpdated(TaskUpdatedMessage {
                task_id: match raw.get("task_id") {
                    None => String::new(),
                    Some(value) => value.as_str()?.to_owned(),
                },
                status: optional_text(&patch, "status")?,
                patch,
                session_id: optional_text(raw, "session_id")?,
                uuid: optional_text(raw, "uuid")?,
            }))
        })(),
        "mirror_error" => (|| {
            Some(SystemEvent::MirrorError(MirrorErrorMessage {
                key: raw.get("key").filter(|value| !value.is_null()).cloned(),
                error: match raw.get("error") {
                    None => String::new(),
                    Some(value) => value.as_str()?.to_owned(),
                },
            }))
        })(),
        _ => None,
    };
    event.unwrap_or(SystemEvent::Other)
}

/// Python: `hook_event_name` is the first truthy of `hook_event`, `hook_name` and `hook_event_name`, else empty.
fn hook_event(raw: &JsonObject) -> Option<HookEventMessage> {
    let name = ["hook_event", "hook_name", "hook_event_name"]
        .into_iter()
        .map(|key| raw.get(key))
        .find(|value| truthy(*value))
        .flatten();
    Some(HookEventMessage {
        hook_event_name: match name {
            None => String::new(),
            Some(value) => value.as_str()?.to_owned(),
        },
        session_id: optional_text(raw, "session_id")?,
        uuid: optional_text(raw, "uuid")?,
    })
}

fn result(raw: &JsonObject) -> Option<ResultMessage> {
    let wire: ResultWire = fields(raw)?;
    Some(ResultMessage {
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
        origin: origin(wire.origin),
        model_usage: wire.model_usage,
        permission_denials: wire.permission_denials,
        deferred_tool_use: wire.deferred_tool_use,
        api_error_status: wire.api_error_status,
        result_index: wire.result_index,
        queued_turn_count: wire.queued_turn_count,
        additional_fields: wire.additional_fields,
        raw: raw.clone(),
    })
}

fn stream_event(raw: &JsonObject) -> Option<StreamEvent> {
    let wire: StreamEventWire = fields(raw)?;
    Some(StreamEvent {
        uuid: wire.uuid,
        session_id: SessionId::from(wire.session_id),
        event: wire.event,
        parent_tool_use_id: wire.parent_tool_use_id.map(ToolUseId::from),
        additional_fields: wire.additional_fields,
        raw: raw.clone(),
    })
}

fn rate_limit(raw: &JsonObject) -> Option<RateLimitEvent> {
    let wire: RateLimitWire = fields(raw)?;
    let info = wire.rate_limit_info;
    Some(RateLimitEvent {
        rate_limit_info: RateLimitInfo {
            status: info.status,
            resets_at: info.resets_at,
            rate_limit_type: info.rate_limit_type,
            utilization: info.utilization,
            overage_status: info.overage_status,
            overage_resets_at: info.overage_resets_at,
            overage_disabled_reason: info.overage_disabled_reason,
            raw: raw.get("rate_limit_info")?.as_object()?.clone(),
        },
        uuid: wire.uuid,
        session_id: SessionId::from(wire.session_id),
        raw: raw.clone(),
    })
}

fn conversation_reset(raw: &JsonObject) -> Option<ConversationResetMessage> {
    let wire: ConversationResetWire = fields(raw)?;
    Some(ConversationResetMessage {
        new_conversation_id: wire.new_conversation_id,
        uuid: wire.uuid,
        session_id: SessionId::from(wire.session_id),
        raw: raw.clone(),
    })
}

#[cfg(test)]
#[path = "message_decode/tests.rs"]
mod tests;
