use crate::{JsonObject, SessionId, ToolName, ToolUseId};

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
