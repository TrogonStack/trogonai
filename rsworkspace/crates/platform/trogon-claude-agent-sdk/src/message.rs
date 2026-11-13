use crate::{JsonObject, MessageId, MessageOrigin, SessionId, ToolName, ToolUseId, upstream::messages as ts};

/// How messages the CLI writes are decoded.
///
/// `TypeScript` (the default) reproduces `@anthropic-ai/claude-agent-sdk`, which validates nothing: every message
/// reaches the caller, a message of an unrecognized type or with an unexpected shape included. A known message
/// whose shape does not fit its Rust struct becomes [`Message::Unknown`], and a content block that does not fit
/// becomes [`ContentBlock::Unknown`], so nothing is dropped.
///
/// `Python` reproduces `claude-agent-sdk`'s `parse_message`: a known message missing a key it requires is a
/// [`crate::MessageParseError`], a message of a type it does not parse is skipped, and so is a content block of a
/// kind it does not parse.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MessageDecoding {
    #[default]
    TypeScript,
    Python,
}

#[derive(Clone, Debug)]
pub enum Message {
    Assistant(AssistantMessage),
    User(UserMessage),
    System(SystemMessage),
    Result(Box<ResultMessage>),
    StreamEvent(StreamEvent),
    /// Python `RateLimitEvent`, TypeScript `SDKRateLimitEvent`.
    RateLimit(RateLimitEvent),
    /// Python `ConversationResetMessage`, TypeScript `SDKConversationResetMessage`.
    ConversationReset(ConversationResetMessage),
    /// A message of a type without a Rust struct above, or a known message whose values do not fit its struct.
    /// `kind` is the `type` (empty when it is absent or not a string), `data` the message without its `type` key
    /// and `raw` the message as written. [`Message::to_sdk_message`] still types every message the pinned
    /// TypeScript declarations list, and [`Message::to_active_goal_message`] the `active_goal` message.
    Unknown {
        kind: String,
        data: JsonObject,
        raw: JsonObject,
    },
}

impl Message {
    /// The message exactly as the CLI wrote it, `type` included.
    pub fn raw(&self) -> &JsonObject {
        match self {
            Self::Assistant(message) => &message.raw,
            Self::User(message) => &message.raw,
            Self::System(message) => &message.raw,
            Self::Result(message) => &message.raw,
            Self::StreamEvent(message) => &message.raw,
            Self::RateLimit(message) => &message.raw,
            Self::ConversationReset(message) => &message.raw,
            Self::Unknown { raw, .. } => raw,
        }
    }

    /// The message typed as TypeScript `SDKMessage`: the member whose `type`, `subtype` and `isReplay` the message
    /// carries, or `SDKMessage::Unknown` with the raw value when none matches exactly. Encoding the result
    /// reproduces [`Message::raw`].
    pub fn to_sdk_message(&self) -> ts::SDKMessage {
        sdk_message(self.raw())
    }

    /// The message typed as TypeScript `SDKActiveGoalMessage`, which the TypeScript `query()` yields next to the
    /// `SDKMessage` members, or `None` when it is not an `active_goal` message of that exact shape. Encoding the
    /// result reproduces [`Message::raw`].
    pub fn to_active_goal_message(&self) -> Option<ts::SDKActiveGoalMessage> {
        match self {
            Self::Unknown { kind, raw, .. } if kind == "active_goal" => exact(raw),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    pub model: String,
    pub parent_tool_use_id: Option<ToolUseId>,
    pub session_id: Option<SessionId>,
    pub error: Option<String>,
    pub usage: Option<JsonObject>,
    pub uuid: Option<MessageId>,
    pub message_id: Option<MessageId>,
    pub stop_reason: Option<String>,
    pub additional_fields: JsonObject,
    pub message_fields: JsonObject,
    /// The message exactly as the CLI wrote it.
    pub raw: JsonObject,
}

impl AssistantMessage {
    /// The message typed as TypeScript `SDKAssistantMessage`, or `None` when it does not match that shape exactly.
    pub fn to_sdk_message(&self) -> Option<ts::SDKAssistantMessage> {
        exact(&self.raw)
    }
}

#[derive(Clone, Debug)]
pub struct UserMessage {
    pub content: Content,
    pub parent_tool_use_id: Option<ToolUseId>,
    pub session_id: Option<SessionId>,
    pub uuid: Option<MessageId>,
    pub origin: Option<MessageOrigin>,
    /// `tool_use_result`: TypeScript `unknown`, Python an optional dictionary that is passed through unchecked.
    /// `None` when the key is absent; JSON `null` is `Some(Value::Null)`.
    pub tool_use_result: Option<serde_json::Value>,
    pub additional_fields: JsonObject,
    pub message_fields: JsonObject,
    /// The message exactly as the CLI wrote it.
    pub raw: JsonObject,
}

impl UserMessage {
    /// `tool_use_result` as the dictionary Python annotates it with, when it is a JSON object.
    pub fn tool_use_result_object(&self) -> Option<&JsonObject> {
        self.tool_use_result.as_ref().and_then(serde_json::Value::as_object)
    }

    /// The message typed as TypeScript `SDKUserMessage` or, with `isReplay: true`, `SDKUserMessageReplay`.
    pub fn to_sdk_message(&self) -> ts::SDKMessage {
        sdk_message(&self.raw)
    }
}

#[derive(Clone, Debug)]
pub enum Content {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Clone, Debug)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: ToolUseId,
        name: ToolName,
        input: JsonObject,
    },
    ToolResult {
        tool_use_id: ToolUseId,
        content: serde_json::Value,
        status: ToolResultStatus,
    },
    /// Python `ServerToolUseBlock`: a tool the API runs itself, such as `advisor`.
    ServerToolUse {
        id: ToolUseId,
        name: String,
        input: JsonObject,
    },
    /// Python `ServerToolResultBlock`, decoded from an `advisor_tool_result` block.
    ServerToolResult {
        tool_use_id: ToolUseId,
        content: serde_json::Value,
    },
    Unknown {
        kind: String,
        data: JsonObject,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolResultStatus {
    NotReported,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct SystemMessage {
    pub subtype: String,
    pub session_id: Option<SessionId>,
    /// The message without its `type` key.
    pub data: JsonObject,
    /// The Python subclass this message decodes to, when there is one.
    pub event: SystemEvent,
    /// The message exactly as the CLI wrote it.
    pub raw: JsonObject,
}

/// The typed views Python `parse_message` gives some system messages. With TypeScript decoding a message that
/// lacks a key Python requires is [`SystemEvent::Other`] instead of an error.
#[derive(Clone, Debug)]
pub enum SystemEvent {
    TaskStarted(TaskStartedMessage),
    TaskProgress(TaskProgressMessage),
    TaskNotification(TaskNotificationMessage),
    TaskUpdated(TaskUpdatedMessage),
    MirrorError(MirrorErrorMessage),
    /// `hook_started` and `hook_response`.
    Hook(HookEventMessage),
    Other,
}

/// Python `TaskStartedMessage`.
#[derive(Clone, Debug)]
pub struct TaskStartedMessage {
    pub task_id: String,
    pub description: String,
    pub uuid: String,
    pub session_id: String,
    pub tool_use_id: Option<String>,
    pub task_type: Option<String>,
}

/// Python `TaskProgressMessage`.
#[derive(Clone, Debug)]
pub struct TaskProgressMessage {
    pub task_id: String,
    pub description: String,
    pub usage: serde_json::Value,
    pub uuid: String,
    pub session_id: String,
    pub tool_use_id: Option<String>,
    pub last_tool_name: Option<String>,
}

/// Python `TaskNotificationMessage`.
#[derive(Clone, Debug)]
pub struct TaskNotificationMessage {
    pub task_id: String,
    pub status: String,
    pub output_file: String,
    pub summary: String,
    pub uuid: String,
    pub session_id: String,
    pub tool_use_id: Option<String>,
    pub usage: Option<serde_json::Value>,
}

/// Python `TaskUpdatedMessage`. Decoded defensively in both modes, as Python does: a missing `task_id` is empty,
/// a `patch` that is not an object is empty, and `status` is the patch's `status`.
#[derive(Clone, Debug)]
pub struct TaskUpdatedMessage {
    pub task_id: String,
    pub patch: JsonObject,
    pub status: Option<String>,
    pub session_id: Option<String>,
    pub uuid: Option<String>,
}

/// Python `MirrorErrorMessage`. A missing `error` is empty.
#[derive(Clone, Debug)]
pub struct MirrorErrorMessage {
    pub key: Option<serde_json::Value>,
    pub error: String,
}

/// Python `HookEventMessage`: `hook_event_name` is the first non-empty string of `hook_event`, `hook_name` and
/// `hook_event_name`, else empty.
#[derive(Clone, Debug)]
pub struct HookEventMessage {
    pub hook_event_name: String,
    pub session_id: Option<String>,
    pub uuid: Option<String>,
}

impl SystemMessage {
    /// The message typed as the TypeScript `SDKMessage` member its `subtype` names, or `SDKMessage::Unknown`.
    pub fn to_sdk_message(&self) -> ts::SDKMessage {
        sdk_message(&self.raw)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultStatus {
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct ResultMessage {
    pub subtype: String,
    pub status: ResultStatus,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub num_turns: u64,
    pub session_id: SessionId,
    pub total_cost_usd: Option<f64>,
    pub result: Option<String>,
    pub structured_output: Option<serde_json::Value>,
    pub usage: Option<JsonObject>,
    pub errors: Option<Vec<String>>,
    pub uuid: Option<MessageId>,
    pub stop_reason: Option<String>,
    pub terminal_reason: Option<String>,
    pub origin: Option<MessageOrigin>,
    pub model_usage: Option<JsonObject>,
    pub permission_denials: Option<Vec<serde_json::Value>>,
    pub deferred_tool_use: Option<JsonObject>,
    pub api_error_status: Option<u16>,
    pub result_index: Option<u64>,
    pub queued_turn_count: Option<u64>,
    pub additional_fields: JsonObject,
    /// The message exactly as the CLI wrote it.
    pub raw: JsonObject,
}

/// Python `DeferredToolUse`.
#[derive(Clone, Debug)]
pub struct DeferredToolUse {
    pub id: String,
    pub name: String,
    pub input: JsonObject,
}

impl ResultMessage {
    /// `deferred_tool_use` as Python's `DeferredToolUse`, when it carries a string `id` and `name` and an object
    /// `input`.
    pub fn deferred_tool(&self) -> Option<DeferredToolUse> {
        let deferred = self.deferred_tool_use.as_ref()?;
        Some(DeferredToolUse {
            id: deferred.get("id")?.as_str()?.to_owned(),
            name: deferred.get("name")?.as_str()?.to_owned(),
            input: deferred.get("input")?.as_object()?.clone(),
        })
    }

    /// The message typed as TypeScript `SDKResultMessage`, or `None` when it does not match a member exactly.
    pub fn to_sdk_message(&self) -> Option<ts::SDKResultMessage> {
        ts::SDKResultMessage::best_match(&serde_json::Value::Object(self.raw.clone())).map(|(_, message)| message)
    }
}

#[derive(Clone, Debug)]
pub struct StreamEvent {
    pub uuid: String,
    pub session_id: SessionId,
    pub event: JsonObject,
    pub parent_tool_use_id: Option<ToolUseId>,
    pub additional_fields: JsonObject,
    /// The message exactly as the CLI wrote it.
    pub raw: JsonObject,
}

impl StreamEvent {
    /// The message typed as TypeScript `SDKPartialAssistantMessage` (`ttft_ms`, `user_message_uuid`,
    /// `user_message_uuids`, `resume_reason` and the rest), or `None` when it does not match that shape exactly.
    pub fn to_sdk_message(&self) -> Option<ts::SDKPartialAssistantMessage> {
        exact(&self.raw)
    }
}

/// Python `RateLimitInfo`.
#[derive(Clone, Debug)]
pub struct RateLimitInfo {
    pub status: String,
    pub resets_at: Option<i64>,
    pub rate_limit_type: Option<String>,
    pub utilization: Option<f64>,
    pub overage_status: Option<String>,
    pub overage_resets_at: Option<i64>,
    pub overage_disabled_reason: Option<String>,
    /// `rate_limit_info` exactly as the CLI wrote it.
    pub raw: JsonObject,
}

/// Python `RateLimitEvent`.
#[derive(Clone, Debug)]
pub struct RateLimitEvent {
    pub rate_limit_info: RateLimitInfo,
    pub uuid: String,
    pub session_id: SessionId,
    /// The message exactly as the CLI wrote it.
    pub raw: JsonObject,
}

impl RateLimitEvent {
    /// The message typed as TypeScript `SDKRateLimitEvent`, or `None` when it does not match that shape exactly.
    pub fn to_sdk_message(&self) -> Option<ts::SDKRateLimitEvent> {
        exact(&self.raw)
    }
}

/// Python `ConversationResetMessage`.
#[derive(Clone, Debug)]
pub struct ConversationResetMessage {
    pub new_conversation_id: String,
    pub uuid: String,
    pub session_id: SessionId,
    /// The message exactly as the CLI wrote it.
    pub raw: JsonObject,
}

impl ConversationResetMessage {
    /// The message typed as TypeScript `SDKConversationResetMessage`, or `None` when it does not match that shape
    /// exactly.
    pub fn to_sdk_message(&self) -> Option<ts::SDKConversationResetMessage> {
        exact(&self.raw)
    }
}

fn sdk_message(raw: &JsonObject) -> ts::SDKMessage {
    let value = serde_json::Value::Object(raw.clone());
    match ts::SDKMessage::best_match(&value) {
        Some((_, message)) => message,
        None => ts::SDKMessage::Unknown(value),
    }
}

fn exact<T: serde::Serialize + serde::de::DeserializeOwned>(raw: &JsonObject) -> Option<T> {
    let value = serde_json::Value::Object(raw.clone());
    let typed: T = serde_json::from_value(value.clone()).ok()?;
    serde_json::to_value(&typed)
        .is_ok_and(|round| round == value)
        .then_some(typed)
}
