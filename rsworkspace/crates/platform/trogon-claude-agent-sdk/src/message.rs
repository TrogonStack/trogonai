use crate::{JsonObject, MessageId, MessageOrigin, SessionId, ToolName, ToolUseId};

#[derive(Clone, Debug)]
pub enum Message {
    Assistant(AssistantMessage),
    User(UserMessage),
    System(SystemMessage),
    Result(Box<ResultMessage>),
    StreamEvent(StreamEvent),
    Unknown { kind: String, data: JsonObject },
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
}

#[derive(Clone, Debug)]
pub struct UserMessage {
    pub content: Content,
    pub parent_tool_use_id: Option<ToolUseId>,
    pub session_id: Option<SessionId>,
    pub uuid: Option<MessageId>,
    pub origin: Option<MessageOrigin>,
    pub tool_use_result: Option<serde_json::Value>,
    pub additional_fields: JsonObject,
    pub message_fields: JsonObject,
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
    pub data: JsonObject,
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
}

#[derive(Clone, Debug)]
pub struct StreamEvent {
    pub uuid: String,
    pub session_id: SessionId,
    pub event: JsonObject,
    pub parent_tool_use_id: Option<ToolUseId>,
    pub additional_fields: JsonObject,
}
