#![doc = include_str!("../README.md")]

pub mod adapters;
pub mod testing;
pub mod upstream;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

mod agent_definition;
mod agent_name;
mod builtin_tool_name;
mod cli_path;
mod client;
mod config_value;
mod constants;
mod control;
mod control_request_id;
mod dialog;
mod dialog_kind;
mod driver;
mod effort_level;
mod environment;
mod error;
mod framing;
mod hooks;
mod mcp_server_name;
mod mcp_server_options;
mod mcp_tool_input_schema;
mod mcp_tool_name;
mod mcp_url;
mod message;
mod message_id;
mod message_origin;
mod model_name;
mod options;
mod output_schema;
mod permission;
mod permission_mode;
mod permission_update;
mod prompt;
mod queue_capacity;
mod sdk_mcp_server;
mod sdk_tool;
mod sdk_tools;
mod sensitive_string;
mod session_entry;
mod session_error;
mod session_id;
mod session_info;
mod session_key;
mod session_path_component;
mod session_project_key;
mod session_prompt;
mod session_resume;
mod session_store;
mod session_subpath;
mod session_summary;
mod setting_source;
mod stderr_tail;
mod task_id;
mod timeout;
mod tool_name;
mod tool_rule;
mod tool_use_id;
mod transcript_mirror;
mod usd_budget;
mod user_input;
mod wire;

pub use agent_definition::{AgentDefinition, AgentExecution, AgentMcpServerSpec};
pub use agent_name::AgentName;
pub use builtin_tool_name::BuiltinToolName;
pub use cli_path::CliPathError;
pub use client::{
    ClaudeSdkClient, ClientHandle, Query, query, query_turn, query_turn_with_adapters, query_with_adapters,
};
pub use config_value::ConfigValue;
pub use control::{
    ConnectionInfo, ContextUsageCategory, ContextUsageResponse, FeatureSupport, InterruptReceipt, McpEnabled,
    McpSetServersResult, McpStatusResponse, RewindFilesResult, RewindMode,
};
pub use control_request_id::ControlRequestId;
pub use dialog::{
    ElicitationFuture, ElicitationHandler, ElicitationMode, ElicitationRequest, ElicitationResponse, UserDialogFuture,
    UserDialogHandler, UserDialogRequest, UserDialogResponse,
};
pub use dialog_kind::DialogKind;
pub use effort_level::EffortLevel;
pub use environment::Environment;
pub use error::{ControlOperation, ControlRejectionError, Error, ProtocolError};
pub use hooks::{
    CallbackError, HookContinuation, HookEvent, HookFailureMode, HookFuture, HookHandler, HookOutput,
    HookOutputVisibility, HookPermissionDecision, HookRegistration, HookRequest, HookSpecificOutput, SyncHookOutput,
};
pub use mcp_server_name::McpServerName;
pub use mcp_server_options::McpServerOptions;
pub use mcp_tool_input_schema::{McpToolInputSchema, McpToolInputSchemaError};
pub use mcp_tool_name::McpToolName;
pub use mcp_url::{McpUrl, McpUrlError};
pub use message::{
    AssistantMessage, Content, ContentBlock, Message, ResultMessage, ResultStatus, StreamEvent, SystemMessage,
    ToolResultStatus, UserMessage,
};
pub use message_id::MessageId;
pub use message_origin::{MessageOrigin, MessageOriginError};
pub use model_name::ModelName;
pub use options::{
    ClaudeAgentOptions, ContinueConversation, Customizations, FileCheckpointing, Limits, McpDiscovery, PartialMessages,
    SessionFork, SessionPersistence, SystemPrompt, ToolSet,
};
pub use output_schema::{OutputSchema, OutputSchemaError};
pub use permission::{
    DenialDisposition, McpServerProvenance, PermissionDecision, PermissionFuture, ToolPermissionHandler,
    ToolPermissionRequest,
};
pub use permission_mode::PermissionMode;
pub use permission_update::{
    PermissionBehavior, PermissionRuleValue, PermissionUpdate, PermissionUpdateDestination, PermissionUpdateError,
};
pub use prompt::Prompt;
pub use queue_capacity::{QueueCapacity, QueueCapacityError};
pub use sdk_mcp_server::{
    LocalMcpServer, LocalMcpServerOptions, McpFuture, McpServerError, SdkMcpServer, SdkMcpSession,
};
pub use sdk_tool::{McpToolFuture, McpToolHandler, McpToolOutput, McpToolRequest, SdkTool, SdkToolOptions};
pub use sdk_tools::{SdkTools, SdkToolsError};
pub use sensitive_string::SensitiveString;
pub use session_entry::{SessionEntry, SessionEntryError};
pub use session_error::SessionError;
pub use session_id::SessionId;
pub use session_info::SdkSessionInfo;
pub use session_key::{SessionKey, SessionKeyError};
pub use session_project_key::{SessionProjectKey, SessionProjectKeyError};
pub use session_resume::{MaterializedSession, SessionPreparationOptions, SessionRuntime, StoreResume};
pub use session_store::{
    InMemorySessionStore, MirrorSizeEstimate, SessionListEntry, SessionStore, SessionStoreError, SessionStoreFlush,
    SessionStoreFuture, SessionStoreLimits, SessionStoreOperation, SessionStoreOptions,
};
pub use session_subpath::{SessionSubpath, SessionSubpathError};
pub use session_summary::{SessionSummaryEntry, SummaryDialect, fold_session_summary, summary_entry_to_sdk_info};
pub use setting_source::SettingSource;
pub use stderr_tail::StderrTail;
pub use task_id::TaskId;
pub use timeout::{Timeout, TimeoutError};
pub use tokio_util::sync::CancellationToken;
pub use tool_name::ToolName;
pub use tool_rule::ToolRule;
pub use tool_use_id::ToolUseId;
pub use transcript_mirror::{MirrorEnqueueError, MirrorFailureError, MirrorFlush, TranscriptMirror};
pub use usd_budget::{UsdBudget, UsdBudgetError};
pub use user_input::{InputBlock, UserInput};

/// A JSON object whose schema is owned by an individual tool or upstream event.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;
