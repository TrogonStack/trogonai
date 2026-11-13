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

/// How messages wait for the caller to read them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MessageBuffer {
    /// TypeScript: an unbounded queue; the CLI is read whether or not the caller keeps up.
    #[default]
    Unbounded,
    /// Python: once this many messages wait, stdout is not read until the caller takes one, so control responses,
    /// callbacks and mirror frames wait too.
    Backpressure(QueueCapacity),
    /// Opt-in: fail the run with [`crate::Error::MessageQueueFull`] when this many messages wait.
    FailWhenFull(QueueCapacity),
}

impl MessageBuffer {
    pub(crate) fn capacity(self) -> Option<QueueCapacity> {
        match self {
            Self::Unbounded => None,
            Self::Backpressure(capacity) | Self::FailWhenFull(capacity) => Some(capacity),
        }
    }
}

/// How long the initialize request may go unanswered.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InitializeTimeout {
    /// TypeScript: 660 seconds, armed only when a hook is set to block on failure or managed settings are sent;
    /// otherwise no limit. Expiry is [`crate::Error::InitializeNotConfirmed`], or
    /// [`crate::Error::ManagedSettingsNotConfirmed`] when only managed settings armed it.
    #[default]
    TypeScript,
    /// Python: `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` milliseconds from the parent environment, at least 60 seconds.
    /// Expiry is [`crate::Error::ControlTimeout`] for the initialize operation.
    Python,
    /// Opt-in: this wait, whatever the hooks; expiry is [`crate::Error::ControlTimeout`].
    Fixed(Timeout),
}

/// How the process is stopped once stdin is closed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Shutdown {
    /// TypeScript: wait 2 seconds, then SIGTERM, then SIGKILL 5 seconds later without waiting for it; stderr gets
    /// 200 ms after the exit to close. Nothing here is an error.
    #[default]
    TypeScript,
    /// Python: wait 5 seconds, SIGTERM, wait 5 seconds, SIGKILL, wait 5 seconds. Nothing here is an error.
    Python,
    /// Opt-in: Python's ladder with this wait at each step, failing with [`crate::Error::ShutdownTimeout`] when the
    /// process outlives it and [`crate::Error::StderrDrainTimeout`] when stderr stays open as long.
    Strict(Timeout),
}

/// What an inbound control request does while another with the same id is in flight.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DuplicateControlRequests {
    /// TypeScript: the duplicate is skipped without a response.
    #[default]
    Skip,
    /// Python: both run and both answer.
    RunBoth,
    /// Opt-in: the run fails with [`crate::ProtocolError::DuplicateControlRequest`].
    Fail,
}

/// What happens when the control request counter runs out.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RequestIdExhaustion {
    /// The counter wraps, so ids never run out, like upstream's random ids.
    #[default]
    Wrap,
    /// Opt-in: the run fails with [`crate::Error::RequestIdExhausted`].
    Fail,
}

/// A prompt sent after [`crate::ClientHandle::finish_input`] while stdin is still open.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InputAfterFinish {
    /// Upstream: written like any other input, extending the run.
    #[default]
    Write,
    /// Opt-in: refused with [`crate::Error::InputClosed`].
    Reject,
}

/// Waits and bounds on the runtime. The default is TypeScript's: no control or write timeout and no bounded queue.
/// [`Limits::python`] selects Python's values. Every other bound is opt-in.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Python's `max_buffer_size`: the longest stdout line, in characters, before
    /// [`crate::Error::JsonDecode`]. TypeScript has no cap.
    pub max_buffer_size: Option<NonZeroUsize>,
    /// UTF-16 code units of stderr kept for the process error. TypeScript keeps 2048; Python keeps none.
    pub stderr_tail: usize,
    pub message_buffer: MessageBuffer,
    /// Opt-in: commands buffered ahead of the runtime. Neither upstream SDK queues them.
    pub command_queue_capacity: Option<QueueCapacity>,
    /// Opt-in: how long a command waits for room in a bounded command queue before
    /// [`crate::Error::CommandQueueTimeout`].
    pub command_queue_timeout: Option<Timeout>,
    /// Opt-in: outstanding outbound control requests before [`crate::Error::TooManyControls`].
    pub max_pending_controls: Option<QueueCapacity>,
    /// Opt-in: inbound SDK callbacks running at once before the next one is refused with an error response.
    pub max_incoming_callbacks: Option<QueueCapacity>,
    /// Opt-in: background tasks tracked for run completion before [`crate::Error::TooManyTasks`].
    pub max_tracked_tasks: Option<QueueCapacity>,
    /// Opt-in: user inputs awaiting a result before [`crate::Error::TooManyInputs`].
    pub max_queued_inputs: Option<QueueCapacity>,
    /// The wait for a control response before [`crate::Error::ControlTimeout`]: none in TypeScript, 60 seconds in
    /// Python.
    pub control_timeout: Option<Timeout>,
    pub initialize_timeout: InitializeTimeout,
    /// Opt-in: how long an inbound SDK callback may run before its timeout answer is sent. Neither upstream SDK
    /// times out callbacks; when set, a hook's own timeout applies to its callbacks instead.
    pub callback_timeout: Option<Timeout>,
    /// Opt-in: how long one stdin write may take before [`crate::Error::WriteTimeout`].
    pub write_timeout: Option<Timeout>,
    pub shutdown: Shutdown,
    /// `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` in both upstream SDKs: with no idle state after the last result, the
    /// run ends this long after it.
    pub run_end_timeout: Timeout,
    pub duplicate_control_requests: DuplicateControlRequests,
    pub request_id_exhaustion: RequestIdExhaustion,
    pub input_after_finish: InputAfterFinish,
}

impl Limits {
    /// TypeScript keeps the last 2048 UTF-16 code units of stderr.
    pub const TYPESCRIPT_STDERR_TAIL: usize = 2048;
    /// Python's `_DEFAULT_MAX_BUFFER_SIZE`.
    pub const PYTHON_MAX_BUFFER_SIZE: NonZeroUsize = match NonZeroUsize::new(1024 * 1024) {
        Some(size) => size,
        None => NonZeroUsize::MIN,
    };

    /// Python's values: a 60-second control wait, a 1 MiB line cap, a 100-message buffer with backpressure, its
    /// initialize wait and shutdown ladder, no stderr tail, and duplicate control requests that all run.
    pub fn python() -> Self {
        Self {
            max_buffer_size: Some(Self::PYTHON_MAX_BUFFER_SIZE),
            stderr_tail: 0,
            message_buffer: MessageBuffer::Backpressure(QueueCapacity::PYTHON_MESSAGES),
            control_timeout: Some(Timeout::default()),
            initialize_timeout: InitializeTimeout::Python,
            shutdown: Shutdown::Python,
            duplicate_control_requests: DuplicateControlRequests::RunBoth,
            ..Self::default()
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_buffer_size: None,
            stderr_tail: Self::TYPESCRIPT_STDERR_TAIL,
            message_buffer: MessageBuffer::Unbounded,
            command_queue_capacity: None,
            command_queue_timeout: None,
            max_pending_controls: None,
            max_incoming_callbacks: None,
            max_tracked_tasks: None,
            max_queued_inputs: None,
            control_timeout: None,
            initialize_timeout: InitializeTimeout::TypeScript,
            callback_timeout: None,
            write_timeout: None,
            shutdown: Shutdown::TypeScript,
            run_end_timeout: Timeout::new(std::time::Duration::from_millis(600_000)),
            duplicate_control_requests: DuplicateControlRequests::Skip,
            request_id_exhaustion: RequestIdExhaustion::Wrap,
            input_after_finish: InputAfterFinish::Write,
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
    /// `thinking` / `thinkingConfig`. Takes precedence over `max_thinking_tokens`.
    pub thinking: Option<crate::ThinkingConfig>,
    /// The deprecated `max_thinking_tokens`. TypeScript treats `0` as disabled thinking; Python forwards it as is.
    pub max_thinking_tokens: Option<u64>,
    pub task_budget: Option<crate::TaskBudget>,
    pub betas: Vec<crate::SdkBeta>,
    pub fallback_model: Option<ModelName>,
    /// The permission prompt tool. Rejected together with `permission_handler`.
    pub permission_prompt_tool_name: Option<String>,
    /// TypeScript `permissionPrompts`. Ignored with `ArgvStyle::Python`.
    pub permission_prompts: Option<crate::PermissionPrompts>,
    /// TypeScript `channels`. Ignored with `ArgvStyle::Python`.
    pub channels: Vec<String>,
    /// TypeScript `debug`. Ignored with `ArgvStyle::Python`.
    pub debug: crate::DebugOutput,
    /// TypeScript `debugFile`. Ignored with `ArgvStyle::Python`.
    pub debug_file: Option<PathBuf>,
    pub include_hook_events: crate::HookEvents,
    pub session_id: Option<SessionId>,
    pub resume_session_at: Option<String>,
    /// Forwarded whenever set, including the empty string, so the CLI rejects a malformed value.
    pub resume_drops_turn: Option<String>,
    /// TypeScript `allowDangerouslySkipPermissions`. Ignored with `ArgvStyle::Python`.
    pub allow_dangerously_skip_permissions: crate::DangerousPermissionSkip,
    /// TypeScript `projectConfigRoot`. Ignored with `ArgvStyle::Python`.
    pub project_config_root: Option<PathBuf>,
    pub skills: Option<crate::Skills>,
    /// `settings`: a settings file path, JSON text or a settings object. Passed as `--settings`; with
    /// `sandbox` set the dialect decides how the two are combined.
    pub settings: Option<crate::SettingsInput>,
    /// `sandbox`: merged into the settings document, as both upstream SDKs do.
    pub sandbox: Option<crate::SandboxSettings>,
    /// TypeScript `managedSettings`. Ignored with `ArgvStyle::Python`.
    pub managed_settings: Option<crate::Settings>,
    /// TypeScript `managedSettingsDelivery`. Ignored with `ArgvStyle::Python`.
    pub managed_settings_delivery: crate::ManagedSettingsDelivery,
}
