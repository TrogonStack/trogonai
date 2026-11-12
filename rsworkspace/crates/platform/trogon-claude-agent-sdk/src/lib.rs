#![doc = include_str!("../README.md")]

pub mod adapters;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

mod agent_name;
mod builtin_tool_name;
mod client;
mod config_value;
mod constants;
mod control_request_id;
mod dialog_kind;
mod effort_level;
mod environment;
mod error;
mod mcp_server_name;
mod mcp_tool_name;
mod message;
mod message_id;
mod message_origin;
mod model_name;
mod options;
mod permission_mode;
mod prompt;
mod queue_capacity;
mod sensitive_string;
mod session_entry;
mod session_error;
mod session_id;
mod session_key;
mod session_path_component;
mod session_project_key;
mod session_resume;
mod session_store;
mod session_subpath;
mod setting_source;
mod stderr_tail;
mod task_id;
mod timeout;
mod tool_name;
mod tool_rule;
mod tool_use_id;
mod transcript_mirror;
mod usd_budget;

pub use agent_name::AgentName;
pub use builtin_tool_name::BuiltinToolName;
pub use client::ClaudeSdkClient;
pub use config_value::ConfigValue;
pub use control_request_id::ControlRequestId;
pub use dialog_kind::DialogKind;
pub use effort_level::EffortLevel;
pub use environment::Environment;
pub use error::Error;
pub use mcp_server_name::McpServerName;
pub use mcp_tool_name::McpToolName;
pub use message::{
    AssistantMessage, Content, ContentBlock, Message, ResultMessage, ResultStatus, StreamEvent, SystemMessage,
    ToolResultStatus, UserMessage,
};
pub use message_id::MessageId;
pub use message_origin::{MessageOrigin, MessageOriginError};
pub use model_name::ModelName;
pub use options::ClaudeAgentOptions;
pub use permission_mode::PermissionMode;
pub use prompt::Prompt;
pub use queue_capacity::{QueueCapacity, QueueCapacityError};
pub use sensitive_string::SensitiveString;
pub use session_entry::{SessionEntry, SessionEntryError};
pub use session_error::SessionError;
pub use session_id::SessionId;
pub use session_key::{SessionKey, SessionKeyError};
pub use session_project_key::{SessionProjectKey, SessionProjectKeyError};
pub use session_resume::{MaterializedSession, SessionPreparationOptions, SessionRuntime, StoreResume};
pub use session_store::{
    InMemorySessionStore, SessionListEntry, SessionStore, SessionStoreError, SessionStoreFuture, SessionStoreLimits,
    SessionStoreOperation, SessionStoreOptions,
};
pub use session_subpath::{SessionSubpath, SessionSubpathError};
pub use setting_source::SettingSource;
pub use stderr_tail::StderrTail;
pub use task_id::TaskId;
pub use timeout::{Timeout, TimeoutError};
pub use tool_name::ToolName;
pub use tool_rule::ToolRule;
pub use tool_use_id::ToolUseId;
pub use transcript_mirror::{MirrorEnqueueError, MirrorFailureError, MirrorFlush, TranscriptMirror};
pub use usd_budget::{UsdBudget, UsdBudgetError};

/// A JSON object whose schema is owned by an individual tool or upstream event.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;
