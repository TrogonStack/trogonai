#![doc = include_str!("../README.md")]

mod agent_name;
mod builtin_tool_name;
mod client;
mod control_request_id;
mod dialog_kind;
mod effort_level;
mod error;
mod mcp_server_name;
mod mcp_tool_name;
mod message;
mod message_id;
mod message_origin;
mod options;
mod permission_mode;
mod prompt;
mod queue_capacity;
mod session_id;
mod setting_source;
mod stderr_tail;
mod task_id;
mod timeout;
mod tool_name;
mod tool_rule;
mod tool_use_id;
mod usd_budget;

pub use client::ClaudeSdkClient;
pub use error::Error;
pub use options::ClaudeAgentOptions;
pub use permission_mode::PermissionMode;
pub use setting_source::SettingSource;

pub use usd_budget::{UsdBudget, UsdBudgetError};

pub use effort_level::EffortLevel;

pub use agent_name::AgentName;

pub use tool_rule::ToolRule;

pub use prompt::Prompt;

pub use session_id::SessionId;

pub use message_id::MessageId;

pub use control_request_id::ControlRequestId;
pub use task_id::TaskId;
pub use tool_use_id::ToolUseId;

pub use tool_name::ToolName;

pub use builtin_tool_name::BuiltinToolName;

pub use mcp_server_name::McpServerName;

pub use mcp_tool_name::McpToolName;

pub use dialog_kind::DialogKind;

pub use queue_capacity::{QueueCapacity, QueueCapacityError};

pub use timeout::{Timeout, TimeoutError};

pub use message::{
    AssistantMessage, Content, ContentBlock, ResultMessage, ResultStatus, StreamEvent, SystemMessage, ToolResultStatus,
    UserMessage,
};
pub use message_origin::{MessageOrigin, MessageOriginError};
pub use stderr_tail::StderrTail;

/// A JSON object whose schema is owned by an individual tool or upstream event.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;
