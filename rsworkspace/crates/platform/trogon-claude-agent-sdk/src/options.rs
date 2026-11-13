use std::{collections::BTreeMap, num::NonZeroUsize, path::PathBuf, sync::Arc};

use crate::{
    AgentDefinition, AgentName, BuiltinToolName, DialogKind, EffortLevel, ElicitationHandler, Environment,
    HookRegistration, McpServerName, McpServerOptions, ModelName, OutputSchema, PermissionMode, QueueCapacity,
    SdkMcpServer, SessionId, SessionStoreOptions, SettingSource, Timeout, ToolPermissionHandler, ToolRule, UsdBudget,
    UserDialogHandler,
    process_options::{
        ArgvStyle, ClaudeCodeMarker, EnvironmentMode, ExtraArgument, ScriptRuntime, StderrHandler, TraceContext,
    },
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ToolSet {
    #[default]
    CliDefault,
    Only(Vec<BuiltinToolName>),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum McpDiscovery {
    /// Passes `--strict-mcp-config`. Upstream sends it only when the caller asks for it.
    Disabled,
    #[default]
    CliDefault,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionPersistence {
    #[default]
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Customizations {
    #[default]
    Configured,
    Bare,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FileCheckpointing {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContinueConversation {
    #[default]
    No,
    Yes,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionFork {
    #[default]
    Continue,
    Fork,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PartialMessages {
    #[default]
    Exclude,
    Include,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SystemPrompt {
    #[default]
    Empty,
    Custom(String),
    ClaudeCode,
    ClaudeCodeWithAppend(String),
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_frame_bytes: NonZeroUsize,
    pub max_stderr_bytes: NonZeroUsize,
    pub message_queue_capacity: QueueCapacity,
    pub command_queue_capacity: QueueCapacity,
    pub max_pending_controls: QueueCapacity,
    pub control_timeout: Timeout,
    pub shutdown_timeout: Timeout,
    pub run_end_timeout: Timeout,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: NonZeroUsize::new(1024 * 1024).unwrap_or(NonZeroUsize::MIN),
            max_stderr_bytes: NonZeroUsize::new(64 * 1024).unwrap_or(NonZeroUsize::MIN),
            message_queue_capacity: QueueCapacity::DEFAULT_MESSAGES,
            command_queue_capacity: QueueCapacity::default(),
            max_pending_controls: QueueCapacity::default(),
            control_timeout: Timeout::default(),
            shutdown_timeout: Timeout::SHUTDOWN_DEFAULT,
            run_end_timeout: Timeout::new(std::time::Duration::from_secs(600)).unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClaudeAgentOptions {
    pub cli_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub model: Option<ModelName>,
    pub max_turns: Option<NonZeroUsize>,
    pub tools: ToolSet,
    pub mcp_discovery: McpDiscovery,
    pub session_persistence: SessionPersistence,
    pub customizations: Customizations,
    pub allowed_tools: Vec<ToolRule>,
    pub disallowed_tools: Vec<ToolRule>,
    pub permission_mode: Option<PermissionMode>,
    pub resume: Option<SessionId>,
    pub partial_messages: PartialMessages,
    pub system_prompt: SystemPrompt,
    /// Filesystem settings to load: `None` inherits CLI defaults; an empty list disables them.
    pub setting_sources: Option<Vec<SettingSource>>,
    pub limits: Limits,
    pub permission_handler: Option<Arc<dyn ToolPermissionHandler>>,
    pub hooks: Vec<HookRegistration>,
    pub agents: BTreeMap<AgentName, AgentDefinition>,
    pub agent: Option<AgentName>,
    pub mcp_servers: BTreeMap<McpServerName, McpServerOptions>,
    pub sdk_mcp_servers: BTreeMap<McpServerName, Arc<dyn SdkMcpServer>>,
    pub max_budget_usd: Option<UsdBudget>,
    pub output_schema: Option<OutputSchema>,
    pub file_checkpointing: FileCheckpointing,
    pub effort: Option<EffortLevel>,
    pub env: Environment,
    pub additional_directories: Vec<PathBuf>,
    pub plugins: Vec<crate::LocalPlugin>,
    /// TypeScript's `pluginDelivery`. Ignored with `ArgvStyle::Python`, which has no such option.
    pub plugin_delivery: crate::PluginDelivery,
    pub continue_conversation: ContinueConversation,
    pub fork_session: SessionFork,
    pub session_store: Option<SessionStoreOptions>,
    pub elicitation_handler: Option<Arc<dyn ElicitationHandler>>,
    pub user_dialog_handler: Option<Arc<dyn UserDialogHandler>>,
    pub supported_dialog_kinds: Vec<DialogKind>,
    pub argv_style: ArgvStyle,
    /// How the messages the CLI writes are decoded: `TypeScript` (the default) yields every message, `Python`
    /// rejects the ones `parse_message` rejects and skips the ones it skips.
    pub message_decoding: crate::MessageDecoding,
    pub environment_mode: EnvironmentMode,
    pub claude_code_marker: ClaudeCodeMarker,
    /// `CLAUDE_CODE_ENTRYPOINT`; `None` selects `sdk-trogon-rs`.
    pub entrypoint: Option<String>,
    /// Application name (optionally `name/version`) reported as `CLAUDE_AGENT_SDK_CLIENT_APP`.
    pub client_app: Option<String>,
    /// Python dialect: derive `CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP` for a bundled CLI. TypeScript derives none.
    pub client_app_derivation: crate::ClientAppDerivation,
    pub trace_context: Option<TraceContext>,
    pub stderr_handler: Option<StderrHandler>,
    pub extra_args: Vec<ExtraArgument>,
    pub script_runtime: ScriptRuntime,
    /// Python's `user`: the CLI runs as this account (Unix only). The name is looked up in `/etc/passwd`; only the
    /// uid changes, as with `Popen(user=...)`. Takes precedence over `run_as_uid`.
    pub user: Option<String>,
    /// Numeric user id the CLI runs as (Unix only), as `Popen(user=int)` does.
    pub run_as_uid: Option<u32>,
}
