#![doc = include_str!("../README.md")]

pub mod adapters;
pub mod testing;
pub mod upstream;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

mod agent_definition;
mod agent_name;
mod argv;
mod builtin_tool_name;
mod bundled_cli;
mod channel;
mod cli_discovery;
mod cli_path;
mod client;
mod client_app;
mod client_app_derivation;
mod config_value;
mod constants;
mod control;
mod control_request_id;
mod debug_file;
mod dialog;
mod dialog_kind;
mod driver;
mod effort_level;
mod environment;
mod environment_plan;
mod error;
mod framing;
mod hooks;
mod managed_settings_delivery;
mod mcp_server_name;
mod mcp_server_options;
mod mcp_tool_input_schema;
mod mcp_tool_name;
mod mcp_url;
mod message;
mod message_decode;
mod message_id;
mod message_origin;
mod model_name;
mod option_error;
mod options;
mod output_schema;
mod permission;
mod permission_mode;
mod permission_update;
mod plugin;
mod process_options;
mod process_transport;
mod prompt;
mod python_json;
mod queue_capacity;
#[cfg(unix)]
mod run_as;
mod runtime_options;
mod sdk_mcp_server;
mod sdk_tool;
mod sdk_tools;
mod sensitive_string;
mod session_entry;
mod session_error;
mod session_files;
mod session_files_ts;
mod session_fork_ts;
mod session_id;
mod session_info;
mod session_key;
mod session_messages_ts;
mod session_mutations;
mod session_path_component;
mod session_precompact;
mod session_probe_ts;
mod session_project_key;
mod session_prompt;
mod session_reader;
mod session_resume;
mod session_store;
mod session_store_helpers;
mod session_store_ts;
mod session_subpath;
mod session_summary;
mod session_transcript;
mod setting_source;
mod settings_dialect;
mod settings_input;
mod settings_json;
mod settings_merge_spec;
mod settings_numbers;
mod settings_option;
mod settings_read;
mod settings_read_hooks;
mod settings_read_spec;
mod settings_resolve;
mod settings_resolve_env;
mod settings_resolve_parent;
mod settings_resolve_platform;
mod settings_resolve_policy;
mod settings_resolve_sources;
mod settings_resolve_tier;
mod settings_resolve_values;
mod settings_sandbox;
mod settings_schema;
mod settings_schema_builtins;
mod settings_schema_hosts;
mod settings_schema_paths;
mod settings_schema_permission_rule;
mod settings_schema_ports;
mod settings_schema_regexp;
mod shadow_warning;
mod skills;
mod stderr_tail;
mod summary_dialect;
mod system_prompt;
mod task_id;
mod timeout;
mod tool_name;
mod tool_rule;
mod tool_use_id;
mod transcript_mirror;
mod transport;
mod usd_budget;
mod user_input;
mod version_check;
mod wire;
mod workspace_trust;

pub use agent_definition::{AgentDefinition, AgentExecution, AgentMcpServerSpec};
pub use agent_name::AgentName;
pub use builtin_tool_name::BuiltinToolName;
pub use bundled_cli::{
    NativeCliNotFoundError, NativePackageTarget, NodeModuleLookup, NodeModuleNotFoundError, ResolveNodeModule,
    bundled_cli_derives_client_app, find_bundled_cli, find_native_cli, meets_derived_client_app_minimum,
};
pub use cli_discovery::CliNotFoundError;
pub use cli_path::{CliPathError, WindowsBatchCli};
pub use client::{
    ClaudeSdkClient, ClientHandle, Query, query, query_turn, query_turn_with_adapters, query_turn_with_transport,
    query_with_adapters, query_with_transport,
};
pub use client_app::{format_client_app, normalize_client_app};
pub use client_app_derivation::{BundledCli, ClientAppDerivation};
pub use config_value::ConfigValue;
pub use constants::{DEFAULT_ENTRYPOINT, MINIMUM_CLAUDE_CODE_VERSION, SDK_VERSION};
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
pub use environment_plan::EnvironmentPlan;
pub use error::{
    AbortError, ControlOperation, ControlRejectionError, Error, JsonDecodeError, JsonDecodeSourceError,
    MessageParseError, ProtocolError, ResultError,
};
pub use hooks::{
    CallbackError, HookContinuation, HookEvent, HookFailureMode, HookFuture, HookHandler, HookOutput,
    HookOutputVisibility, HookPermissionDecision, HookRegistration, HookRequest, HookSpecificOutput, SyncHookOutput,
};
pub use managed_settings_delivery::ManagedSettingsDelivery;
pub use mcp_server_name::McpServerName;
pub use mcp_server_options::McpServerOptions;
pub use mcp_tool_input_schema::{McpToolInputSchema, McpToolInputSchemaError};
pub use mcp_tool_name::McpToolName;
pub use mcp_url::{McpUrl, McpUrlError};
pub use message::{
    AssistantMessage, Content, ContentBlock, ConversationResetMessage, DeferredToolUse, HookEventMessage, Message,
    MessageDecoding, MirrorErrorMessage, RateLimitEvent, RateLimitInfo, ResultMessage, ResultStatus, StreamEvent,
    SystemEvent, SystemMessage, TaskNotificationMessage, TaskProgressMessage, TaskStartedMessage, TaskUpdatedMessage,
    ToolResultStatus, UserMessage,
};
pub use message_id::MessageId;
pub use message_origin::{MessageOrigin, MessageOriginError};
pub use model_name::ModelName;
pub use option_error::OptionError;
pub use options::{
    ClaudeAgentOptions, ContinueConversation, Customizations, DuplicateControlRequests, FileCheckpointing,
    InitializeTimeout, InputAfterFinish, Limits, McpDiscovery, MessageBuffer, PartialMessages, RequestIdExhaustion,
    SessionFork, SessionPersistence, Shutdown, SystemPrompt, ToolSet, VerbatimPrompts,
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
pub use plugin::{LocalPlugin, PluginDelivery};
pub use process_options::{
    ArgvStyle, ClaudeCodeMarker, EnvironmentMode, ExtraArgument, ScriptRuntime, StderrDelivery, StderrHandler,
    TraceContext,
};
pub use prompt::Prompt;
pub use queue_capacity::{QueueCapacity, QueueCapacityError};
pub use runtime_options::{
    DangerousPermissionSkip, DebugOutput, HookEvents, PermissionPrompts, SdkBeta, Skills, TaskBudget, ThinkingConfig,
    ThinkingDisplay,
};
pub use sdk_mcp_server::{
    LocalMcpServer, LocalMcpServerOptions, McpFuture, McpServerError, SdkMcpServer, SdkMcpSession,
};
pub use sdk_tool::{McpToolFuture, McpToolHandler, McpToolOutput, McpToolRequest, SdkTool, SdkToolOptions};
pub use sdk_tools::{SdkTools, SdkToolsError};
pub use sensitive_string::SensitiveString;
pub use session_entry::{SessionEntry, SessionEntryError};
pub use session_error::SessionError;
pub use session_files::{ListSessionsOptions, SessionFiles};
pub use session_id::SessionId;
pub use session_info::SdkSessionInfo;
pub use session_key::{SessionKey, SessionKeyError};
pub use session_messages_ts::{ChainAlgorithm, MessageOptions, MessagesError};
pub use session_mutations::{
    ForkAlgorithm, ForkSessionResult, JsonEscaping, SessionMutationError, delete_session, fork_session, rename_session,
    tag_session,
};
pub use session_project_key::{
    SessionProjectKey, SessionProjectKeyError, project_key_for_directory, project_key_for_directory_in,
};
pub use session_reader::{
    get_session_info, get_session_messages, get_subagent_messages, list_sessions, list_subagents,
};
pub use session_resume::{MaterializedSession, SessionPreparationOptions, SessionRuntime, StoreResume};
pub use session_store::{
    InMemorySessionStore, MirrorSizeEstimate, SessionListEntry, SessionStore, SessionStoreError, SessionStoreFlush,
    SessionStoreFuture, SessionStoreLimits, SessionStoreOperation, SessionStoreOptions,
};
pub use session_store_helpers::{
    ImportOptions, SessionStoreHelperError, StoreListOptions, delete_session_via_store, delete_session_via_store_with,
    fork_session_via_store, fork_session_via_store_with, get_session_info_from_store, get_session_info_from_store_with,
    get_session_messages_from_store, get_session_messages_from_store_with, get_subagent_messages_from_store,
    get_subagent_messages_from_store_with, import_session_to_store, list_sessions_from_store,
    list_subagents_from_store, list_subagents_from_store_with, rename_session_via_store, rename_session_via_store_with,
    tag_session_via_store, tag_session_via_store_with,
};
pub use session_subpath::{SessionSubpath, SessionSubpathError};
pub use session_summary::{SessionSummaryEntry, fold_session_summary, summary_entry_to_sdk_info};
pub use session_transcript::{MessageExtras, MessagePage, SessionMessage, SessionMessageKind};
pub use setting_source::SettingSource;
pub use settings_dialect::SettingsDialect;
pub use settings_input::SettingsInput;
pub use settings_resolve::{
    ResolveSettingsError, filter_escalating_default_mode, resolve_settings, resolve_settings_in_dialect,
    resolve_settings_in_dialect_with, resolve_settings_with,
};
pub use settings_sandbox::{
    SandboxCredentialsConfig, SandboxFilesystemConfig, SandboxIgnoreViolations, SandboxNetworkConfig, SandboxSettings,
};
pub use stderr_tail::StderrTail;
pub use summary_dialect::SummaryDialect;
pub use system_prompt::{CustomPrompt, PresetPrompt};
pub use task_id::TaskId;
pub use timeout::{Timeout, TimeoutError};
pub use tokio_util::sync::CancellationToken;
pub use tool_name::ToolName;
pub use tool_rule::ToolRule;
pub use tool_use_id::ToolUseId;
pub use transcript_mirror::{MirrorEnqueueError, MirrorFailureError, MirrorFlush, TranscriptMirror};
pub use transport::Transport;
pub use upstream::settings::Settings;
pub use upstream::settings_resolution::{
    PolicySettingsOrigin, ProvenanceEntry, ResolveSettingsOptions, ResolvedSettingSource, ResolvedSettings,
    SDKSettingsParseError,
};
pub use usd_budget::{UsdBudget, UsdBudgetError};
pub use user_input::{InputBlock, UserInput};
pub use version_check::{parse_version, unreported_version_warning};
pub use workspace_trust::WorkspaceTrust;

/// A JSON object whose schema is owned by an individual tool or upstream event.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;
