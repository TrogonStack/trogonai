use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::{Future, pending},
    sync::Arc,
    time::Duration,
};

use tokio::{
    sync::oneshot,
    task::{AbortHandle, JoinHandle, JoinSet},
    time::{Instant, sleep_until, timeout},
};

use crate::{
    ClaudeAgentOptions, ControlOperation, ControlRejectionError, ControlRequestId, Error, JsonObject, Limits, Message,
    PermissionDecision, ProtocolError, StderrTail, ToolPermissionHandler, Transport,
    constants::{HOST_AUTH_CALLBACK_MISSING, OAUTH_CALLBACK_MISSING, TYPESCRIPT_INITIALIZE_TIMEOUT},
    wire::{
        self, ControlResponse, Frame, IncomingControl,
        runtime_payload::{
            EmbeddedMcpPayload, HookMatcherPayload, HooksPayload, InitializeRequest, McpResponsePayload, RequestPayload,
        },
    },
};

pub(crate) enum Command {
    /// TypeScript `initialize({first})`: the connect-time request, or `reinitialize()` resending it.
    Initialize { first: bool, response: ControlSender },
    SetMcpServers {
        external: crate::McpServers,
        embedded: crate::SdkMcpServers,
        response: ControlSender,
    },
    Prompt {
        input: crate::UserInput,
        response: oneshot::Sender<Result<(), Error>>,
    },
    Control {
        operation: ControlOperation,
        request: serde_json::Value,
        response: ControlSender,
    },
    /// A control whose `response` field is returned as sent, `None` when the frame has none.
    RawControl {
        operation: ControlOperation,
        request: serde_json::Value,
        response: ControlSender,
    },
    FinishInput {
        completion: Completion,
        response: oneshot::Sender<Result<(), Error>>,
    },
    /// TypeScript `abortController.abort()`: stop the process now and end the run with an `AbortError`.
    Abort,
}

#[derive(Clone, Copy)]
pub(crate) enum Completion {
    Turn,
    Run,
}

#[derive(Default)]
struct RunState {
    input_done: Option<Completion>,
    result_received: bool,
    turn_active: bool,
    session_state: Option<SessionState>,
    tasks: HashSet<String>,
    awaiting_continuation: bool,
    deadline: Option<Instant>,
    pending_inputs: VecDeque<Option<crate::MessageId>>,
}

#[derive(Clone, Copy)]
enum SessionState {
    Idle,
    Running,
    RequiresAction,
    /// A state this SDK does not know yet. It is treated as not idle.
    Unknown,
}

impl RunState {
    fn observe(
        &mut self,
        message: &Message,
        queued_limit: Option<usize>,
        task_limit: Option<usize>,
    ) -> Result<(), Error> {
        match message {
            Message::Result(message) => {
                self.result_received = true;
                self.turn_active = false;
                self.awaiting_continuation = false;
                if let Some(count) = message.queued_turn_count {
                    let count = usize::try_from(count).unwrap_or(usize::MAX);
                    if queued_limit.is_some_and(|limit| count > limit) {
                        return Err(Error::TooManyInputs);
                    }
                    while self.pending_inputs.len() > count {
                        self.pending_inputs.pop_front();
                    }
                    if count > 0 {
                        self.awaiting_continuation = true;
                    }
                } else {
                    self.pending_inputs.pop_front();
                }
            }
            Message::Assistant(message) if message.parent_tool_use_id.is_none() => {
                self.turn_active = true;
                self.deadline = None;
            }
            Message::StreamEvent(message) if message.parent_tool_use_id.is_none() => {
                self.turn_active = true;
                self.deadline = None;
            }
            Message::System(message) => match message.subtype.as_str() {
                "session_state_changed" => {
                    self.session_state = Some(match message.data.get("state").and_then(serde_json::Value::as_str) {
                        Some("idle") => SessionState::Idle,
                        Some("running") => SessionState::Running,
                        Some("requires_action") => SessionState::RequiresAction,
                        _ => SessionState::Unknown,
                    });
                    if matches!(self.session_state, Some(SessionState::Idle)) && self.result_received {
                        self.awaiting_continuation = false;
                    }
                    if matches!(self.session_state, Some(SessionState::RequiresAction)) {
                        self.deadline = None;
                    }
                }
                "task_started"
                    if matches!(
                        message.data.get("task_type").and_then(serde_json::Value::as_str),
                        Some("local_agent" | "local_workflow")
                    ) =>
                {
                    if let Some(id) = message.data.get("task_id").and_then(serde_json::Value::as_str) {
                        if task_limit.is_some_and(|limit| self.tasks.len() >= limit) && !self.tasks.contains(id) {
                            return Err(Error::TooManyTasks);
                        }
                        self.tasks.insert(id.to_owned());
                    }
                }
                "task_notification" | "task_updated" => {
                    let terminal = message.subtype == "task_notification"
                        || matches!(
                            message
                                .data
                                .get("patch")
                                .and_then(|value| value.get("status"))
                                .and_then(serde_json::Value::as_str),
                            Some("completed" | "failed" | "stopped" | "killed")
                        );
                    if terminal
                        && let Some(id) = message.data.get("task_id").and_then(serde_json::Value::as_str)
                        && self.tasks.remove(id)
                    {
                        self.awaiting_continuation = true;
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    fn settled(&self, callbacks: usize) -> bool {
        self.result_received
            && match self.input_done {
                Some(Completion::Turn) => callbacks == 0,
                Some(Completion::Run) => {
                    !self.turn_active
                        && self.pending_inputs.is_empty()
                        && callbacks == 0
                        && self.tasks.is_empty()
                        && !self.awaiting_continuation
                        && matches!(self.session_state, None | Some(SessionState::Idle))
                }
                None => false,
            }
    }
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;

struct PendingControl {
    response: ControlResponder,
    /// `None` waits for the answer however long it takes, as TypeScript does.
    deadline: Option<Instant>,
}

/// Receives a control success's `response` field, `None` when the frame has none (TypeScript `undefined`).
pub(crate) type ControlSender = oneshot::Sender<Result<Option<serde_json::Value>, Error>>;

enum ControlResponder {
    Initialize {
        first: bool,
        response: ControlSender,
    },
    Control {
        operation: ControlOperation,
        response: ControlSender,
    },
    Raw {
        operation: ControlOperation,
        response: ControlSender,
    },
}

impl ControlResponder {
    fn operation(&self) -> ControlOperation {
        match self {
            Self::Initialize { .. } => ControlOperation::Initialize,
            Self::Control { operation, .. } | Self::Raw { operation, .. } => *operation,
        }
    }

    fn send_error(self, error: Error) {
        match self {
            Self::Initialize { response, .. } | Self::Control { response, .. } | Self::Raw { response, .. } => {
                let _ = response.send(Err(error));
            }
        }
    }
}

/// The response a callback writes; `None` when the request stays unanswered.
type CallbackOutput = (ControlRequestId, Result<Option<serde_json::Value>, ProtocolError>);

type McpSessions = indexmap::IndexMap<crate::McpServerName, Arc<dyn crate::SdkMcpSession>>;

enum State {
    Initializing,
    Idle,
    Running,
}

struct Driver<T, F = crate::adapters::NativeFileSystem>
where
    T: Transport,
    F: crate::adapters::SessionFileSystem,
{
    transport: T,
    /// `end_input` has been called, so nothing more is written whatever `Transport::is_ready` says.
    input_ended: bool,
    commands: crate::channel::Receiver<Command>,
    messages: crate::channel::Sender<Message>,
    /// Python's backpressure: messages the caller has not taken yet beyond the buffer. While any wait here,
    /// stdout is not read.
    overflow: VecDeque<Message>,
    shutdown: oneshot::Receiver<()>,
    /// The shutdown signal has been taken, so the receiver must not be polled again.
    shutdown_seen: bool,
    limits: Limits,
    handler: Option<Arc<dyn ToolPermissionHandler>>,
    pending: HashMap<ControlRequestId, PendingControl>,
    callbacks: JoinSet<CallbackOutput>,
    callback_handles: HashMap<ControlRequestId, AbortHandle>,
    callback_signals: HashMap<ControlRequestId, crate::CancellationToken>,
    /// Python: callbacks that lost their request id to a duplicate still run and answer, but a cancel no longer
    /// reaches them.
    detached_callbacks: HashMap<tokio::task::Id, crate::CancellationToken>,
    hook_callbacks: HashMap<String, HookCallback>,
    sdk_mcp_servers: McpSessions,
    initialize_request: serde_json::Value,
    initialize_expectations: InitializeExpectations,
    initialize_timeout: Option<Duration>,
    elicitation_handler: Option<Arc<dyn crate::ElicitationHandler>>,
    user_dialog_handler: Option<Arc<dyn crate::UserDialogHandler>>,
    oauth_token_handler: Option<Arc<dyn crate::OAuthTokenHandler>>,
    host_auth_token_handler: Option<Arc<dyn crate::HostAuthTokenHandler>>,
    dialog_kinds: HashSet<String>,
    session: Option<crate::SessionRuntime<F>>,
    mirror_flush: Option<MirrorBarrier>,
    deferred_messages: VecDeque<(Message, Option<MirrorBarrier>)>,
    next_id: u64,
    state: State,
    run_state: RunState,
    argv_style: crate::ArgvStyle,
    message_decoding: crate::MessageDecoding,
    /// The last `is_error` result, kept while nothing else has arrived since, to turn the exit error into a
    /// result error the way both upstream SDKs do.
    last_error_result: Option<JsonObject>,
    /// The CLI path until the first `system`/`init` message has been checked, then `None`.
    version_check: Option<String>,
    latest_commands: LatestCommands,
    /// `verbatim_prompts`: every user message is marked `client_composed`.
    verbatim_prompts: bool,
    /// Python warns at the first init message when the CLI predates `client_composed`; TypeScript does not.
    warn_unsupported_verbatim: bool,
}

type MirrorBarrier =
    std::pin::Pin<Box<dyn Future<Output = Result<Vec<crate::MirrorFailureError>, crate::SessionError>> + Send>>;

#[derive(Clone)]
struct HookCallback {
    handler: Arc<dyn crate::HookHandler>,
    timeout: Option<Duration>,
}

/// TypeScript's `pluginsViaInitialize`: `pluginDelivery: 'initialize'` with at least one plugin.
pub(crate) fn plugins_via_initialize(options: &ClaudeAgentOptions) -> bool {
    options.argv_style == crate::ArgvStyle::TypeScript
        && options.plugin_delivery == crate::PluginDelivery::Initialize
        && !options.plugins.is_empty()
}

/// What the initialize response must confirm, from what the request carried.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InitializeExpectations {
    /// A hook matcher sets `onFailure` to anything but `continue`.
    pub blocking_hooks: bool,
    /// The request carried `managedSettings`.
    pub managed_settings: bool,
    /// How many plugins the request listed.
    pub plugins: usize,
}

/// Both SDKs: a whole number of seconds is sent as an integer (`30`, not `30.0`).
fn hook_timeout_seconds(timeout: Duration) -> Option<serde_json::Number> {
    if timeout.subsec_nanos() == 0 {
        Some(timeout.as_secs().into())
    } else {
        serde_json::Number::from_f64(timeout.as_secs_f64())
    }
}

fn strings(values: &[String]) -> serde_json::Value {
    serde_json::Value::Array(values.iter().cloned().map(serde_json::Value::String).collect())
}

/// The `response` a control success carries: Python reads anything but an object, an absent one included, as
/// `{}`; TypeScript keeps the value as sent and an absent one as `undefined`, which is `None` here.
fn control_value(style: crate::ArgvStyle, response: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match (style, response) {
        (crate::ArgvStyle::Python, Some(serde_json::Value::Object(object))) => Some(serde_json::Value::Object(object)),
        (crate::ArgvStyle::Python, _) => Some(serde_json::Value::Object(JsonObject::new())),
        (crate::ArgvStyle::TypeScript, response) => response,
    }
}

/// The command list the CLI announced last, which TypeScript `supportedCommands()` prefers to the initialize one.
pub(crate) type LatestCommands = Arc<std::sync::Mutex<Option<serde_json::Value>>>;

/// JavaScript `String(value)` of an optional boolean.
fn javascript_string(support: crate::FeatureSupport) -> &'static str {
    match support {
        crate::FeatureSupport::Supported => "true",
        crate::FeatureSupport::Unsupported => "false",
        crate::FeatureSupport::NotReported => "undefined",
    }
}

fn initialize_config(
    options: &ClaudeAgentOptions,
) -> Result<(serde_json::Value, HashMap<String, HookCallback>, InitializeExpectations), Error> {
    if !options.supported_dialog_kinds.is_empty() && options.user_dialog_handler.is_none() {
        return Err(Error::DialogKindsWithoutHandler);
    }
    crate::shadow_warning::reject_handler_with_tool_name(options)?;
    let typescript = options.argv_style == crate::ArgvStyle::TypeScript;
    let mut hooks = HooksPayload::new();
    let mut callbacks = HashMap::new();
    let mut expectations = InitializeExpectations::default();
    for registration in &options.hooks {
        let mut ids = Vec::new();
        for handler in &registration.callbacks {
            let id = format!("hook_{}", callbacks.len());
            ids.push(id.clone());
            callbacks.insert(
                id,
                HookCallback {
                    handler: handler.clone(),
                    // Both upstream SDKs only forward the hook timeout to the CLI; the SDK enforces one only when
                    // the caller opted into a callback timeout.
                    timeout: options
                        .limits
                        .callback_timeout
                        .map(|limit| registration.timeout.unwrap_or(limit).as_duration()),
                },
            );
        }
        let matcher = match (&registration.matcher, typescript) {
            (Some(matcher), _) => Some(Some(matcher.as_str())),
            (None, true) => None,
            (None, false) => Some(None),
        };
        hooks
            .entry(registration.event.as_str())
            .or_default()
            .push(HookMatcherPayload {
                matcher,
                callback_ids: ids,
                timeout: registration
                    .timeout
                    .and_then(|timeout| hook_timeout_seconds(timeout.as_duration())),
                on_failure: registration.on_failure.map(crate::HookFailureMode::as_str),
            });
        expectations.blocking_hooks |= registration
            .on_failure
            .is_some_and(|mode| mode != crate::HookFailureMode::Continue);
    }
    let agents = if options.agents.is_empty() {
        None
    } else {
        Some(
            options
                .agents
                .iter()
                .map(|(name, definition)| definition.to_wire_json().map(|value| (name.as_str().to_owned(), value)))
                .collect::<Result<JsonObject, _>>()
                .map_err(ProtocolError::from)?,
        )
    };
    let supported_dialog_kinds = (!options.supported_dialog_kinds.is_empty()).then(|| {
        options
            .supported_dialog_kinds
            .iter()
            .map(crate::DialogKind::as_str)
            .collect()
    });
    // TypeScript delivers the system prompt in the initialize request, Python on the command line and only the
    // two flags the Python projection keeps.
    let prompt = if options.argv_style == crate::ArgvStyle::Python {
        options.system_prompt.python_initialize()
    } else {
        options.system_prompt.typescript_initialize()
    };
    let flag = |value: Option<bool>| value.map(serde_json::Value::Bool);
    let text = |value: &Option<String>| value.clone().map(serde_json::Value::String);
    // TypeScript names its in-process MCP servers in `initialize`; Python only lists them in `--mcp-config`.
    let sdk_mcp_servers = (typescript && !options.sdk_mcp_servers.is_empty()).then(|| {
        options
            .sdk_mcp_servers
            .keys()
            .map(crate::McpServerName::as_str)
            .collect()
    });
    let request = InitializeRequest {
        hooks: (!options.hooks.is_empty()).then_some(hooks),
        sdk_mcp_servers,
        json_schema: options.output_schema.as_ref().map(crate::OutputSchema::as_json),
        system_prompt: prompt.system_prompt,
        append_system_prompt: prompt.append_system_prompt,
        plan_mode_instructions: text(&options.plan_mode_instructions),
        system_prompt_snapshot: flag(prompt.snapshot),
        append_subagent_system_prompt: text(&options.append_subagent_system_prompt),
        tool_aliases: options.tool_aliases.as_ref().map(|aliases| {
            serde_json::Value::Object(
                aliases
                    .iter()
                    .map(|(name, target)| (name.clone(), serde_json::Value::String(target.clone())))
                    .collect(),
            )
        }),
        code_running_mcp_tools: options.code_running_mcp_tools.as_deref().map(strings),
        exclude_dynamic_sections: flag(prompt.exclude_dynamic_sections),
        agents,
        title: text(&options.title),
        skills: crate::skills::initialize_skills(options),
        web_search_isolation_exempt_mcp_servers: options
            .web_search_isolation_exempt_mcp_servers
            .as_deref()
            .map(strings),
        prompt_suggestions: flag(options.prompt_suggestions),
        agent_progress_summaries: flag(options.agent_progress_summaries),
        forward_subagent_text: flag(options.forward_subagent_text),
        supported_dialog_kinds,
        per_task_stop_affordance: flag(options.per_task_stop_affordance),
        rapid_followup_preempt: flag(options.rapid_followup_preempt),
        forward_nested_subagent_tool_calls: flag(options.forward_nested_subagent_tool_calls),
        workspace_trust: options
            .workspace_trust
            .as_ref()
            .and_then(crate::WorkspaceTrust::validated)
            .map(|trust| serde_json::Value::Object(trust.to_object())),
        plugins: plugins_via_initialize(options)
            .then(|| options.plugins.iter().map(crate::plugin::PluginPayload::from).collect()),
        managed_settings: crate::settings_option::initialize_managed_settings(options)?,
        ..InitializeRequest::default()
    };
    expectations.managed_settings = request.managed_settings.is_some();
    expectations.plugins = request.plugins.as_ref().map_or(0, Vec::len);
    let request = request.into_payload(options.argv_style).to_value()?;
    Ok((request, callbacks, expectations))
}

pub(crate) struct SpawnOptions<T, F: crate::adapters::SessionFileSystem> {
    pub options: ClaudeAgentOptions,
    pub session: Option<crate::SessionRuntime<F>>,
    pub commands: crate::channel::Receiver<Command>,
    pub messages: crate::channel::Sender<Message>,
    pub shutdown: oneshot::Receiver<()>,
    pub terminal: oneshot::Sender<Result<(), Error>>,
    pub transport: T,
    pub initialization: Initialization,
    /// The CLI path to check against the minimum version, for a transport that runs the CLI itself.
    pub version_check: Option<String>,
    pub latest_commands: LatestCommands,
}

/// The initialize request and the callbacks it registers, built before any transport exists so that a bad
/// configuration fails before a process is spawned.
pub(crate) struct Initialization {
    request: serde_json::Value,
    hook_callbacks: HashMap<String, HookCallback>,
    expectations: InitializeExpectations,
    timeout: Option<Duration>,
}

impl Initialization {
    pub(crate) fn new<E: crate::adapters::EnvironmentVariable>(
        options: &ClaudeAgentOptions,
        environment: &E,
    ) -> Result<Self, Error> {
        let (request, hook_callbacks, expectations) = initialize_config(options)?;
        let timeout = match options.limits.initialize_timeout {
            // TypeScript arms `AE + T8` (660 seconds) only when a hook blocks on failure or managed settings are sent.
            crate::InitializeTimeout::TypeScript => {
                (expectations.blocking_hooks || expectations.managed_settings).then_some(TYPESCRIPT_INITIALIZE_TIMEOUT)
            }
            crate::InitializeTimeout::Python => Some(python_initialize_timeout(
                environment.var_os(std::ffi::OsStr::new("CLAUDE_CODE_STREAM_CLOSE_TIMEOUT")),
            )?),
            crate::InitializeTimeout::Fixed(timeout) => Some(timeout.as_duration()),
        };
        Ok(Self {
            request,
            hook_callbacks,
            expectations,
            timeout,
        })
    }
}

/// Python: `max(int(os.environ.get("CLAUDE_CODE_STREAM_CLOSE_TIMEOUT", "60000")) / 1000, 60)` seconds.
pub(crate) fn python_initialize_timeout(value: Option<std::ffi::OsString>) -> Result<Duration, Error> {
    let floor = Duration::from_secs(60);
    let Some(value) = value else {
        return Ok(floor);
    };
    let text = value.to_string_lossy();
    let milliseconds = python_int(&text).ok_or_else(|| Error::InvalidStreamCloseTimeout {
        literal: python_repr(&text),
    })?;
    Ok(match milliseconds {
        PythonInt::Negative => floor,
        PythonInt::Positive(milliseconds) => Duration::from_millis(milliseconds).max(floor),
    })
}

enum PythonInt {
    Negative,
    /// Saturated at `u64::MAX`, which is longer than any wait.
    Positive(u64),
}

/// The values Python's `int(text)` accepts in base 10: surrounding whitespace, one sign, and ASCII digits with
/// single underscores between them.
fn python_int(text: &str) -> Option<PythonInt> {
    let text = text.trim();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut value = 0_u64;
    for byte in digits.bytes() {
        match byte {
            b'_' => {}
            b'0'..=b'9' => value = value.saturating_mul(10).saturating_add(u64::from(byte - b'0')),
            _ => return None,
        }
    }
    Some(if negative && value > 0 {
        PythonInt::Negative
    } else {
        PythonInt::Positive(value)
    })
}

/// Python's `repr` of a string, as its `int()` error quotes it.
fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut repr = String::with_capacity(text.len() + 2);
    repr.push(quote);
    for character in text.chars() {
        match character {
            '\\' => repr.push_str("\\\\"),
            '\n' => repr.push_str("\\n"),
            '\r' => repr.push_str("\\r"),
            '\t' => repr.push_str("\\t"),
            character if character == quote => {
                repr.push('\\');
                repr.push(character);
            }
            character if u32::from(character) < 0x20 || character == '\u{7f}' => {
                let code = u32::from(character);
                repr.push_str("\\x");
                repr.extend(
                    [code >> 4, code & 0xf]
                        .into_iter()
                        .filter_map(|digit| char::from_digit(digit, 16)),
                );
            }
            character => repr.push(character),
        }
    }
    repr.push(quote);
    repr
}

/// Connects the transport and starts the driver task.
pub(crate) async fn spawn<T: Transport, F: crate::adapters::SessionFileSystem>(
    input: SpawnOptions<T, F>,
) -> Result<JoinHandle<()>, Error> {
    let SpawnOptions {
        options,
        session,
        commands,
        messages,
        shutdown,
        terminal,
        mut transport,
        initialization,
        version_check,
        latest_commands,
    } = input;
    transport.connect().await?;
    let driver = Driver {
        transport,
        input_ended: false,
        commands,
        messages,
        overflow: VecDeque::new(),
        shutdown,
        shutdown_seen: false,
        limits: options.limits,
        handler: options.permission_handler,
        pending: HashMap::new(),
        callbacks: JoinSet::new(),
        callback_handles: HashMap::new(),
        callback_signals: HashMap::new(),
        detached_callbacks: HashMap::new(),
        hook_callbacks: initialization.hook_callbacks,
        sdk_mcp_servers: options
            .sdk_mcp_servers
            .iter()
            .map(|(name, server)| (name.clone(), server.open_session()))
            .collect(),
        initialize_request: initialization.request,
        initialize_expectations: initialization.expectations,
        initialize_timeout: initialization.timeout,
        elicitation_handler: options.elicitation_handler,
        user_dialog_handler: options.user_dialog_handler,
        oauth_token_handler: options.oauth_token_handler,
        host_auth_token_handler: options.host_auth_token_handler,
        dialog_kinds: options
            .supported_dialog_kinds
            .iter()
            .map(|kind| kind.as_str().to_owned())
            .collect(),
        session,
        mirror_flush: None,
        deferred_messages: VecDeque::new(),
        next_id: 0,
        state: State::Initializing,
        run_state: RunState::default(),
        argv_style: options.argv_style,
        message_decoding: options.message_decoding,
        last_error_result: None,
        version_check,
        latest_commands,
        verbatim_prompts: options.verbatim_prompts == crate::VerbatimPrompts::Enabled,
        warn_unsupported_verbatim: options.verbatim_prompts == crate::VerbatimPrompts::Enabled
            && options.argv_style == crate::ArgvStyle::Python,
    };
    Ok(tokio::spawn(driver.run(terminal)))
}

impl<T, F> Driver<T, F>
where
    T: Transport,
    F: crate::adapters::SessionFileSystem,
{
    async fn run(mut self, terminal: oneshot::Sender<Result<(), Error>>) {
        let primary = match self.event_loop().await {
            Err(Error::ShutdownRequested) => Ok(()),
            outcome => outcome,
        };
        self.callbacks.abort_all();
        for signal in self.callback_signals.values().chain(self.detached_callbacks.values()) {
            signal.cancel();
        }
        self.end_input().await;
        let immediate = primary.is_err() && !matches!(primary, Err(Error::UnexpectedEof));
        let killed = if immediate { self.transport.kill().await } else { Ok(()) };
        let closed = match killed {
            Ok(()) => self.transport.close().await,
            Err(error) => {
                let _ = self.transport.close().await;
                Err(error)
            }
        };
        let mirror_cleanup = self.finalize_mirror().await;
        if let Some(materialized) = self.session.as_mut().and_then(|session| session.materialized.take()) {
            materialized.cleanup().await;
        }
        let outcome = match closed {
            Ok(()) => primary,
            Err(Error::Process { status, stderr }) => match primary {
                Ok(()) | Err(Error::UnexpectedEof) => Err(self.exit_error(status, stderr)),
                Err(error) => Err(error),
            },
            Err(cleanup) => match primary {
                Ok(()) => Err(cleanup),
                Err(primary) => Err(Error::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                }),
            },
        };
        let outcome = combine_outcomes(outcome, mirror_cleanup);
        self.flush_overflow().await;
        for (_, pending) in self.pending.drain() {
            pending.response.send_error(Error::DriverStopped);
        }
        self.commands.close();
        drop(self.messages);
        let _ = terminal.send(outcome);
    }

    /// Whether another document may be written: the transport is ready and input has not been ended.
    fn input_open(&self) -> bool {
        !self.input_ended && self.transport.is_ready()
    }

    /// Python `end_input`. A failure to end input is not an error of the run.
    async fn end_input(&mut self) {
        if !self.input_ended {
            self.input_ended = true;
            if let Err(error) = self.transport.end_input().await {
                tracing::debug!(%error, "the transport could not end its input");
            }
        }
    }

    /// Decodes one object the transport yielded. A frame or message that cannot be decoded is a
    /// `MessageParseError` carrying the raw object.
    fn decode_message(&self, object: JsonObject) -> Result<Frame, Error> {
        wire::parse_value(&object, self.message_decoding)
            .map_err(|error| crate::MessageParseError::new(error, Some(object)).into())
    }

    /// The error for an unsuccessful exit: the last error result when one is pending (Python always, TypeScript
    /// when its text is non-empty), otherwise the bare process error.
    fn exit_error(&mut self, status: std::process::ExitStatus, stderr: StderrTail) -> Error {
        let exit_code = crate::error::exit_code(&status);
        if let Some(data) = self.last_error_result.take() {
            let text = match self.argv_style {
                crate::ArgvStyle::Python => Some(crate::error::python_error_result_text(&data)),
                crate::ArgvStyle::TypeScript => crate::error::typescript_error_result_text(&data),
            };
            if let Some(text) = text {
                let message = format!("Claude Code returned an error result: {text}");
                return crate::ResultError::new(message, data, exit_code).into();
            }
        }
        Error::Process { status, stderr }
    }

    async fn event_loop(&mut self) -> Result<(), Error> {
        loop {
            if self.shutdown.try_recv().is_ok() {
                self.shutdown_seen = true;
                return Ok(());
            }
            if let Some(error) = self.expired_control() {
                return Err(error);
            }
            if self.run_state.settled(self.running_callbacks()) && self.mirror_flush.is_none() {
                self.end_input().await;
            }
            let between_turns = self.run_state.input_done.is_some()
                && self.run_state.result_received
                && !self.run_state.turn_active
                && self.running_callbacks() == 0
                && self.run_state.tasks.is_empty()
                && !matches!(self.run_state.session_state, Some(SessionState::RequiresAction));
            if between_turns && self.input_open() {
                if self.run_state.deadline.is_none() {
                    self.run_state.deadline = Instant::now().checked_add(self.limits.run_end_timeout.as_duration());
                }
            } else {
                self.run_state.deadline = None;
            }
            let deadline = self.pending.values().filter_map(|request| request.deadline).min();
            let run_deadline = self.run_state.deadline;
            // Python stops reading stdout while its message buffer is full, so nothing else on stdout is handled
            // until the caller takes a message.
            let reading = self.overflow.is_empty() && !self.deferred_full();
            tokio::select! {
                _ = &mut self.shutdown => { self.shutdown_seen = true; return Ok(()); }
                ready = self.messages.ready(), if !self.overflow.is_empty() => {
                    if !ready { return Err(Error::DriverStopped); }
                    self.drain_overflow()?;
                }
                message = self.transport.read_message(), if reading => {
                    let Some(message) = message? else {
                        self.finish_mirror().await?;
                        return if matches!(self.state, State::Running | State::Initializing) || !self.pending.is_empty() { Err(Error::UnexpectedEof) } else { Ok(()) };
                    };
                    let parsed = self.decode_message(message)?;
                    self.frame(parsed).await?;
                }
                flushed = async { match self.mirror_flush.as_mut() { Some(flush) => flush.await, None => pending().await } }, if self.mirror_flush.is_some() => {
                    self.mirror_flush = None;
                    let failures = flushed?;
                    for failure in failures { self.publish(failure.into_message().map_err(ProtocolError::from)?)?; }
                    self.publish_deferred()?;
                }
                command = self.commands.recv() => {
                    let Some(command) = command else { return Ok(()); };
                    self.command(command).await?;
                }
                callback = self.callbacks.join_next_with_id(), if !self.callbacks.is_empty() => {
                    match callback {
                        Some(Ok((task_id, (id, response)))) if self.callback_handles.get(&id).is_some_and(|handle| handle.id() == task_id) => {
                            self.callback_handles.remove(&id);
                            self.callback_signals.remove(&id);
                            self.write_callback_response(response).await?;
                        }
                        Some(Ok((task_id, (id, response)))) if self.detached_callbacks.remove(&task_id).is_some() => {
                            // Python's `finally` pops the request id whichever task holds it, so the live task
                            // under that id loses its cancel route too.
                            if let Some(handle) = self.callback_handles.remove(&id) {
                                let signal = self.callback_signals.remove(&id).unwrap_or_default();
                                self.detached_callbacks.insert(handle.id(), signal);
                            }
                            self.write_callback_response(response).await?;
                        }
                        Some(Err(error)) if error.is_cancelled() => {}
                        Some(Err(error)) if self.detached_callbacks.contains_key(&error.id()) || self.callback_handles.values().any(|handle| handle.id() == error.id()) => return Err(Error::CallbackTask(error)),
                        _ => {}
                    }
                }
                _ = async { match deadline { Some(deadline) => sleep_until(deadline).await, None => pending::<()>().await } } => {
                    if let Some(error) = self.expired_control() { return Err(error); }
                }
                _ = async { match run_deadline { Some(deadline) => sleep_until(deadline).await, None => pending::<()>().await } } => {
                    tracing::debug!(timeout = ?self.limits.run_end_timeout.as_duration(), "no idle state after the last result; ending the run");
                    self.run_state.deadline = None;
                    self.end_input().await;
                }
            }
        }
    }

    fn running_callbacks(&self) -> usize {
        self.callback_handles.len() + self.detached_callbacks.len()
    }

    /// The control request whose wait ran out, answered with its timeout error, which also ends the run.
    fn expired_control(&mut self) -> Option<Error> {
        let now = Instant::now();
        let id = self.pending.iter().find_map(|(id, request)| {
            request
                .deadline
                .is_some_and(|deadline| deadline <= now)
                .then(|| id.clone())
        })?;
        let request = self.pending.remove(&id)?;
        let operation = request.response.operation();
        let typescript_initialize = operation == ControlOperation::Initialize
            && self.limits.initialize_timeout == crate::InitializeTimeout::TypeScript;
        // The hook message wins when both a blocking hook and managed settings armed the wait.
        let expectations = self.initialize_expectations;
        let error = || {
            if typescript_initialize && !expectations.blocking_hooks && expectations.managed_settings {
                Error::ManagedSettingsNotConfirmed
            } else if typescript_initialize {
                Error::InitializeNotConfirmed
            } else {
                Error::ControlTimeout { operation }
            }
        };
        request.response.send_error(error());
        Some(error())
    }

    fn control_deadline(&self, operation: ControlOperation) -> Option<Instant> {
        let wait = match operation {
            ControlOperation::Initialize => self.initialize_timeout,
            _ => self.limits.control_timeout.map(crate::Timeout::as_duration),
        }?;
        Instant::now().checked_add(wait)
    }

    /// TypeScript sends a random id per request, Python `req_{counter}_{random}`. The counter wraps unless the
    /// caller opted into failing at exhaustion.
    fn next_request_id(&mut self) -> Result<ControlRequestId, Error> {
        self.next_id = match self.limits.request_id_exhaustion {
            crate::RequestIdExhaustion::Wrap => self.next_id.wrapping_add(1),
            crate::RequestIdExhaustion::Fail => self.next_id.checked_add(1).ok_or(Error::RequestIdExhausted)?,
        };
        Ok(match self.argv_style {
            crate::ArgvStyle::TypeScript => ControlRequestId::typescript(),
            crate::ArgvStyle::Python => ControlRequestId::python(self.next_id),
        })
    }

    fn controls_full(&self) -> bool {
        self.limits
            .max_pending_controls
            .is_some_and(|limit| self.pending.len() >= limit.get())
    }

    async fn admit_callback(&mut self, id: &ControlRequestId) -> Result<bool, Error> {
        if let Some(handle) = self.callback_handles.get(id) {
            match self.limits.duplicate_control_requests {
                crate::DuplicateControlRequests::Skip => {
                    // TypeScript logs and drops a second delivery of a request still in flight.
                    tracing::debug!(request_id = %id.as_str(), "skipping a duplicate delivery of an in-flight control request");
                    return Ok(false);
                }
                crate::DuplicateControlRequests::RunBoth => {
                    let task = handle.id();
                    self.callback_handles.remove(id);
                    let signal = self.callback_signals.remove(id).unwrap_or_default();
                    self.detached_callbacks.insert(task, signal);
                }
                crate::DuplicateControlRequests::Fail => return Err(ProtocolError::DuplicateControlRequest.into()),
            }
        }
        if self
            .limits
            .max_incoming_callbacks
            .is_some_and(|limit| self.running_callbacks() >= limit.get())
        {
            self.write_response(wire::callback_error_response(id, "Too many outstanding SDK callbacks"))
                .await?;
            return Ok(false);
        }
        Ok(true)
    }

    async fn command(&mut self, command: Command) -> Result<(), Error> {
        match command {
            Command::Initialize { first, response } => {
                let mut request = self.initialize_request.clone();
                // TypeScript captures SDK MCP server manifests for the first initialize only.
                if !first && let Some(object) = request.as_object_mut() {
                    object.remove("sdkMcpServerManifests");
                }
                self.start_control(ControlResponder::Initialize { first, response }, request)
                    .await?;
            }
            Command::SetMcpServers {
                external,
                embedded,
                response,
            } => {
                if !self.input_open() {
                    let _ = response.send(Err(Error::NotReadyForWriting));
                    return Ok(());
                }
                if self.controls_full() {
                    let _ = response.send(Err(Error::TooManyControls));
                    return Ok(());
                }
                // TypeScript `setMcpServers`: in-process servers are swapped before the request is sent, kept
                // ones keep their session, and every registered one is listed after the external configs,
                // replacing an external config of the same name in place like an object spread.
                let mut servers = serde_json::Map::new();
                for (name, config) in external {
                    servers.insert(
                        name.as_str().to_owned(),
                        config.to_wire_json().map_err(ProtocolError::from)?,
                    );
                }
                let mut sessions = McpSessions::new();
                for (name, registry) in embedded {
                    servers.insert(
                        name.as_str().to_owned(),
                        EmbeddedMcpPayload::Sdk { name: name.as_str() }.to_value()?,
                    );
                    let session = self
                        .sdk_mcp_servers
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| registry.open_session());
                    sessions.insert(name, session);
                }
                self.sdk_mcp_servers = sessions;
                let id = self.next_request_id()?;
                let frame = wire::control_request(
                    &id,
                    RequestPayload::McpSetServers { servers }.to_value()?,
                    self.argv_style,
                )?;
                self.write(frame).await?;
                self.pending.insert(
                    id,
                    PendingControl {
                        response: ControlResponder::Control {
                            operation: ControlOperation::McpSetServers,
                            response,
                        },
                        deadline: self.control_deadline(ControlOperation::McpSetServers),
                    },
                );
            }
            Command::Prompt { input, response } => {
                if !self.input_open() {
                    let _ = response.send(Err(Error::NotReadyForWriting));
                    return Ok(());
                }
                if self.run_state.input_done.is_some()
                    && self.limits.input_after_finish == crate::InputAfterFinish::Reject
                {
                    let _ = response.send(Err(Error::InputClosed));
                    return Ok(());
                }
                match self.state {
                    State::Initializing => {
                        let _ = response.send(Err(Error::NotInitialized));
                        return Ok(());
                    }
                    State::Idle | State::Running => {}
                }
                if self
                    .limits
                    .max_queued_inputs
                    .is_some_and(|limit| self.run_state.pending_inputs.len() >= limit.get())
                {
                    let _ = response.send(Err(Error::TooManyInputs));
                    return Ok(());
                }
                let mut frame = input.to_wire_json().map_err(ProtocolError::from)?;
                if self.verbatim_prompts
                    && let Some(object) = frame.as_object_mut()
                {
                    object.insert("client_composed".into(), true.into());
                }
                self.write(frame).await?;
                self.state = State::Running;
                self.run_state.result_received = false;
                self.run_state.turn_active = true;
                self.run_state.pending_inputs.push_back(input.uuid.clone());
                let _ = response.send(Ok(()));
            }
            Command::Control {
                operation,
                request,
                response,
            } => {
                self.start_control(ControlResponder::Control { operation, response }, request)
                    .await?;
            }
            Command::RawControl {
                operation,
                request,
                response,
            } => {
                self.start_control(ControlResponder::Raw { operation, response }, request)
                    .await?;
            }
            Command::FinishInput { completion, response } => {
                self.run_state.input_done = Some(completion);
                let _ = response.send(Ok(()));
            }
            Command::Abort => return Err(crate::AbortError::PROCESS.into()),
        }
        Ok(())
    }

    /// Applies the initialize checks of `sdk.mjs` `Query.initialize({first})`, in its order. A refusal ends the
    /// session, after a `reinitialize()` too.
    fn finish_initialize(
        &mut self,
        first: bool,
        sender: ControlSender,
        response: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        let response = control_value(self.argv_style, response);
        let connection_info = crate::ConnectionInfo::from_response(response.clone());
        let expectations = self.initialize_expectations;
        let refusal: Option<fn() -> Error> = if expectations.blocking_hooks
            && connection_info.hook_on_failure_supported != crate::FeatureSupport::Supported
        {
            Some(|| Error::BlockingHooksUnsupported)
        } else if expectations.blocking_hooks
            && first
            && connection_info.hooks_applied == crate::FeatureSupport::Unsupported
        {
            Some(|| Error::HooksNotApplied)
        } else if expectations.managed_settings
            && connection_info.managed_settings_applied != crate::FeatureSupport::Supported
        {
            Some(|| Error::ManagedSettingsNotApplied)
        } else {
            None
        };
        if let Some(error) = refusal {
            let _ = sender.send(Err(error()));
            return Err(error());
        }
        if expectations.plugins > 0 && connection_info.plugins_applied != crate::FeatureSupport::Supported {
            tracing::warn!(
                plugins_applied = javascript_string(connection_info.plugins_applied),
                plugins = expectations.plugins,
                "[Query.initialize] Claude Code did not apply the plugins sent with pluginDelivery 'initialize'; the process is running with the plugins it was launched with."
            );
        }
        if first {
            self.state = State::Idle;
        }
        let _ = sender.send(Ok(response));
        Ok(())
    }

    async fn start_control(&mut self, response: ControlResponder, request: serde_json::Value) -> Result<(), Error> {
        if !self.input_open() {
            response.send_error(Error::NotReadyForWriting);
            return Ok(());
        }
        if self.controls_full() {
            response.send_error(Error::TooManyControls);
            return Ok(());
        }
        let id = self.next_request_id()?;
        self.write(wire::control_request(&id, request, self.argv_style)?)
            .await?;
        let deadline = self.control_deadline(response.operation());
        self.pending.insert(id, PendingControl { response, deadline });
        Ok(())
    }

    async fn frame(&mut self, frame: Frame) -> Result<(), Error> {
        match frame {
            Frame::KeepAlive => {}
            // Python clears the pending error result for every message it does not parse.
            Frame::Unparsed => self.last_error_result = None,
            Frame::Message(message) => {
                let (queued_limit, task_limit) = (
                    self.limits.max_queued_inputs.map(crate::QueueCapacity::get),
                    self.limits.max_tracked_tasks.map(crate::QueueCapacity::get),
                );
                if let Message::System(system) = &message
                    && system.subtype == "init"
                    && let Some(cli_path) = self.version_check.take()
                {
                    let version = system
                        .data
                        .get("claude_code_version")
                        .and_then(serde_json::Value::as_str);
                    if let Some(warning) = crate::version_check::unreported_version_warning(version, &cli_path) {
                        tracing::warn!(%warning, "Claude Code version warning");
                    }
                    if self.warn_unsupported_verbatim
                        && let Some(warning) = crate::version_check::unsupported_verbatim_warning(version, &cli_path)
                    {
                        tracing::warn!(%warning, "Claude Code version warning");
                    }
                }
                match &message {
                    Message::Result(result) if result.status == crate::ResultStatus::Error => {
                        self.last_error_result = Some(crate::error::error_result_data(result));
                    }
                    Message::System(system) if system.subtype == "session_state_changed" => {}
                    _ => self.last_error_result = None,
                }
                if let Message::System(system) = &message
                    && system.subtype == "commands_changed"
                    && let Some(commands @ serde_json::Value::Array(_)) = system.raw.get("commands")
                {
                    *self
                        .latest_commands
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(commands.clone());
                }
                self.run_state.observe(&message, queued_limit, task_limit)?;
                // Both SDKs drop the state changes the CLI sent only because the SDK asked for them.
                if let Message::System(system) = &message
                    && system.subtype == "session_state_changed"
                    && system.raw.get("sdk_host_only") == Some(&serde_json::Value::Bool(true))
                {
                    return Ok(());
                }
                if matches!(message, Message::Result(_)) {
                    self.state = State::Idle;
                }
                let mut barrier = if matches!(message, Message::Result(_)) {
                    self.session.as_ref().map(|session| {
                        let marker = session.mirror.begin_flush();
                        Box::pin(async move { marker?.wait().await }) as MirrorBarrier
                    })
                } else {
                    None
                };
                if self.mirror_flush.is_none() {
                    self.mirror_flush = barrier.take();
                }
                if self.mirror_flush.is_some() {
                    if let crate::MessageBuffer::FailWhenFull(capacity) = self.limits.message_buffer
                        && self.deferred_messages.len() >= capacity.get()
                    {
                        return Err(Error::MessageQueueFull);
                    }
                    self.deferred_messages.push_back((message, barrier));
                } else {
                    self.publish(message)?;
                }
            }
            Frame::ControlResponse { id, response } => {
                let Some(pending) = self.pending.remove(&id) else {
                    // One line on purpose: with the `log` feature of `tracing` enabled by another workspace
                    // member, the macro repeats each field expression for the log fallback, and the
                    // fallback copy on a line of its own is never executed, which reads as uncovered.
                    // The `%` form keeps rustfmt from splitting the call.
                    tracing::debug!(request_id = %id.as_str(), "ignoring control response for an unknown request");
                    return Ok(());
                };
                let operation = pending.response.operation();
                match response {
                    ControlResponse::Success(response) => {
                        let (response, sender) = match pending.response {
                            ControlResponder::Initialize {
                                first,
                                response: sender,
                            } => {
                                return self.finish_initialize(first, sender, response);
                            }
                            ControlResponder::Raw { response: sender, .. } => {
                                let _ = sender.send(Ok(response));
                                return Ok(());
                            }
                            ControlResponder::Control { response: sender, .. } => {
                                (control_value(self.argv_style, response), sender)
                            }
                        };
                        if operation == ControlOperation::Interrupt
                            && let Some(object) = response.as_ref().and_then(serde_json::Value::as_object)
                            && let crate::InterruptReceipt::Reported {
                                cancelled: Some(cancelled),
                                ..
                            } = crate::InterruptReceipt::from_json(object.clone())
                        {
                            self.run_state
                                .pending_inputs
                                .retain(|input| input.as_ref().is_none_or(|input| !cancelled.contains(input)));
                        }
                        let _ = sender.send(Ok(response));
                    }
                    ControlResponse::Error { message, error_code } => {
                        pending.response.send_error(Error::ControlRejected {
                            operation,
                            rejection: ControlRejectionError::new(message, error_code, self.argv_style),
                        });
                    }
                }
            }
            Frame::ControlRequest { id, request } => match (request, self.user_dialog_handler.clone()) {
                (IncomingControl::UserDialog { kind, .. }, _) if !self.dialog_kinds.contains(&kind) => {}
                (IncomingControl::UserDialog { .. }, None) => {}
                (IncomingControl::Unsupported, _) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    self.write_response(wire::unsupported_response(&id)).await?;
                }
                (IncomingControl::Malformed(error), _) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    tracing::warn!(
                        ?error,
                        "answering a malformed permission request with an error response"
                    );
                    self.write_response(wire::malformed_response(&id)).await?;
                }
                (IncomingControl::Permission(request), _) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    let style = self.argv_style;
                    let input = request.input.clone();
                    let tool_use_id = request.tool_use_id.clone();
                    let signal = request.signal.clone();
                    if let Some(handler) = self.handler.clone() {
                        let callback_id = id.clone();
                        let fallback = wire::permission_response(
                            &id,
                            input.clone(),
                            tool_use_id.as_ref(),
                            PermissionDecision::deny("Permission callback timed out"),
                            style,
                        );
                        self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                            let decision = handler.decide(*request).await;
                            wire::permission_response(&callback_id, input, tool_use_id.as_ref(), decision, style)
                        });
                    } else {
                        self.write_callback_response(wire::permission_response(
                            &id,
                            input,
                            tool_use_id.as_ref(),
                            PermissionDecision::deny("No permission handler is available"),
                            style,
                        ))
                        .await?;
                    }
                }
                (
                    IncomingControl::Hook {
                        callback_id,
                        input,
                        tool_use_id,
                    },
                    _,
                ) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    if let Some(callback) = self.hook_callbacks.get(&callback_id).cloned() {
                        let signal = crate::CancellationToken::new();
                        let request = crate::HookRequest {
                            input,
                            tool_use_id,
                            signal: signal.clone(),
                        };
                        let response_id = id.clone();
                        let fallback = wire::callback_error_response(&id, "SDK hook callback timed out").map(Some);
                        self.spawn_callback(id, signal, callback.timeout, fallback, async move {
                            match callback.handler.call(request).await {
                                Ok(output) => wire::success_response(&response_id, output.to_wire_json()?).map(Some),
                                Err(_) => {
                                    wire::callback_error_response(&response_id, "SDK hook callback failed").map(Some)
                                }
                            }
                        });
                    } else {
                        self.write_response(wire::callback_error_response(&id, "Unknown SDK hook callback"))
                            .await?;
                    }
                }
                (IncomingControl::Mcp { server_name, message }, _) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    let name = crate::McpServerName::from(server_name);
                    let server = self.sdk_mcp_servers.get(&name).cloned();
                    if let Some(server) = server {
                        let signal = crate::CancellationToken::new();
                        let callback_signal = signal.clone();
                        let response_id = id.clone();
                        let fallback = wire::callback_error_response(&id, "SDK MCP request timed out").map(Some);
                        self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                            match server.request(message, callback_signal).await {
                                Ok(response) => {
                                    wire::success_response(&response_id, McpResponsePayload::new(response)?).map(Some)
                                }
                                Err(_) => {
                                    wire::callback_error_response(&response_id, "SDK MCP request failed").map(Some)
                                }
                            }
                        });
                    } else {
                        self.write_response(wire::callback_error_response(&id, "Unknown SDK MCP server"))
                            .await?;
                    }
                }
                (IncomingControl::Elicitation(request), _) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    if let Some(handler) = self.elicitation_handler.clone() {
                        let signal = crate::CancellationToken::new();
                        let mode = match request.mode.as_deref() {
                            None | Some("form") => crate::ElicitationMode::Form,
                            Some("url") => crate::ElicitationMode::Url,
                            _ => {
                                return self
                                    .write_response(wire::callback_error_response(&id, "Unsupported elicitation mode"))
                                    .await;
                            }
                        };
                        let request = crate::ElicitationRequest {
                            server_name: crate::McpServerName::from(request.server_name),
                            message: request.message,
                            mode,
                            url: request.url,
                            elicitation_id: request.elicitation_id,
                            requested_schema: request.requested_schema,
                            title: request.title,
                            display_name: request.display_name,
                            description: request.description,
                            signal: signal.clone(),
                        };
                        let response_id = id.clone();
                        let fallback = wire::success_response(
                            &id,
                            crate::ElicitationResponse::Cancel
                                .to_wire_json()
                                .map_err(ProtocolError::from)?,
                        )
                        .map(Some);
                        self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                            let response = handler
                                .call(request)
                                .await
                                .unwrap_or(crate::ElicitationResponse::Cancel);
                            wire::success_response(&response_id, response.to_wire_json()?).map(Some)
                        });
                    } else {
                        self.write_response(wire::success_response(
                            &id,
                            crate::ElicitationResponse::Cancel
                                .to_wire_json()
                                .map_err(ProtocolError::from)?,
                        ))
                        .await?;
                    }
                }
                (
                    IncomingControl::UserDialog {
                        kind,
                        payload,
                        tool_use_id,
                    },
                    Some(handler),
                ) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    let signal = crate::CancellationToken::new();
                    let request = crate::UserDialogRequest {
                        dialog_kind: crate::DialogKind::from(kind),
                        payload,
                        tool_use_id,
                        signal: signal.clone(),
                    };
                    let response_id = id.clone();
                    let fallback = wire::success_response(
                        &id,
                        crate::UserDialogResponse::Cancelled
                            .to_wire_json()
                            .map_err(ProtocolError::from)?,
                    )
                    .map(Some);
                    self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                        let response = handler
                            .call(request)
                            .await
                            .unwrap_or(crate::UserDialogResponse::Cancelled);
                        wire::success_response(&response_id, response.to_wire_json()?).map(Some)
                    });
                }
                (IncomingControl::OAuthTokenRefresh { kind, token_state }, _) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    let Some(handler) = self.oauth_token_handler.clone() else {
                        self.write_response(wire::callback_error_response(&id, OAUTH_CALLBACK_MISSING))
                            .await?;
                        return Ok(());
                    };
                    let signal = crate::CancellationToken::new();
                    let request = crate::OAuthTokenRequest {
                        kind,
                        token_state: crate::OAuthTokenState::from_wire(token_state.as_deref()),
                        signal: signal.clone(),
                    };
                    let response_id = id.clone();
                    let fallback = wire::callback_error_response(&id, "getOAuthToken callback timed out").map(Some);
                    self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                        match handler.call(request).await {
                            Ok(response) => wire::success_response(&response_id, response.to_wire_json()).map(Some),
                            Err(error) => {
                                wire::callback_error_response(&response_id, &callback_failure(&error)).map(Some)
                            }
                        }
                    });
                }
                (IncomingControl::HostAuthTokenRefresh { status }, _) => {
                    if !self.admit_callback(&id).await? {
                        return Ok(());
                    }
                    let Some(handler) = self.host_auth_token_handler.clone() else {
                        self.write_response(wire::callback_error_response(&id, HOST_AUTH_CALLBACK_MISSING))
                            .await?;
                        return Ok(());
                    };
                    let signal = crate::CancellationToken::new();
                    let request = crate::HostAuthTokenRequest {
                        status,
                        signal: signal.clone(),
                    };
                    let response_id = id.clone();
                    let fallback = wire::callback_error_response(&id, "getHostAuthToken callback timed out").map(Some);
                    self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                        match handler.call(request).await {
                            Ok(token) => {
                                wire::success_response(&response_id, crate::auth_token::host_auth_wire_json(&token))
                                    .map(Some)
                            }
                            Err(error) => {
                                wire::callback_error_response(&response_id, &callback_failure(&error)).map(Some)
                            }
                        }
                    });
                }
            },
            Frame::Cancel { id } => {
                if let Some(handle) = self.callback_handles.remove(&id) {
                    if let Some(signal) = self.callback_signals.remove(&id) {
                        signal.cancel();
                    }
                    handle.abort();
                }
            }
            Frame::TranscriptMirror { file_path, entries } => {
                if let Some(session) = &self.session {
                    match session.mirror.enqueue(&file_path, entries) {
                        Ok(()) => {}
                        Err(crate::MirrorEnqueueError::Key(error)) => {
                            // One line for the same reason as the control response above.
                            let projects_dir = session.mirror.projects_dir();
                            tracing::warn!(file_path = %file_path.display(), projects_dir = %projects_dir.display(), %error, "dropping transcript mirror frame whose file path is not under the projects directory");
                        }
                        Err(crate::MirrorEnqueueError::Closed) => {
                            return Err(Error::Session(crate::SessionError::MirrorStopped));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn publish(&mut self, message: Message) -> Result<(), Error> {
        if !self.overflow.is_empty() {
            self.overflow.push_back(message);
            return Ok(());
        }
        match self.messages.try_send(message) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(Error::DriverStopped),
            Err(tokio::sync::mpsc::error::TrySendError::Full(message)) => match self.limits.message_buffer {
                crate::MessageBuffer::FailWhenFull(_) => Err(Error::MessageQueueFull),
                crate::MessageBuffer::Unbounded | crate::MessageBuffer::Backpressure(_) => {
                    self.overflow.push_back(message);
                    Ok(())
                }
            },
        }
    }

    /// Moves waiting messages into the buffer while it has room.
    fn drain_overflow(&mut self) -> Result<(), Error> {
        while let Some(message) = self.overflow.pop_front() {
            match self.messages.try_send(message) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(message)) => {
                    self.overflow.push_front(message);
                    break;
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return Err(Error::DriverStopped),
            }
        }
        Ok(())
    }

    /// Hands every waiting message to the caller before the run's outcome, waiting for room as the caller reads.
    /// A shutdown request drops them, as Python's cancelled reader does.
    async fn flush_overflow(&mut self) {
        while let Some(message) = self.overflow.pop_front() {
            if self.shutdown_seen {
                self.overflow.clear();
                return;
            }
            let delivered = tokio::select! {
                _ = &mut self.shutdown => { self.shutdown_seen = true; false }
                sent = self.messages.send(message) => sent.is_ok(),
            };
            if !delivered {
                self.overflow.clear();
            }
        }
    }

    /// Under backpressure, messages held behind a transcript flush count against the buffer too.
    fn deferred_full(&self) -> bool {
        matches!(self.limits.message_buffer, crate::MessageBuffer::Backpressure(capacity) if self.deferred_messages.len() >= capacity.get())
    }

    fn callback_timeout(&self) -> Option<Duration> {
        self.limits.callback_timeout.map(crate::Timeout::as_duration)
    }

    fn publish_deferred(&mut self) -> Result<(), Error> {
        while let Some((message, barrier)) = self.deferred_messages.pop_front() {
            if let Some(barrier) = barrier {
                self.mirror_flush = Some(barrier);
                self.deferred_messages.push_front((message, None));
                break;
            }
            self.publish(message)?;
        }
        Ok(())
    }

    async fn finish_mirror(&mut self) -> Result<(), Error> {
        while let Some(flush) = self.mirror_flush.take() {
            for failure in flush.await? {
                self.publish(failure.into_message().map_err(ProtocolError::from)?)?;
            }
            self.publish_deferred()?;
        }
        Ok(())
    }

    async fn finalize_mirror(&mut self) -> Result<(), Error> {
        let mut outcome = Ok(());
        let mut failures = Vec::new();
        if let Some(barrier) = self.mirror_flush.take() {
            match barrier.await {
                Ok(reported) => failures.extend(reported),
                Err(error) => outcome = Err(Error::Session(error)),
            }
        }
        while let Some((_, barrier)) = self.deferred_messages.pop_front() {
            if let Some(barrier) = barrier {
                match barrier.await {
                    Ok(reported) => failures.extend(reported),
                    Err(error) => outcome = combine_outcomes(outcome, Err(Error::Session(error))),
                }
            }
        }
        if let Some(session) = &self.session {
            match session.mirror.flush().await {
                Ok(reported) => failures.extend(reported),
                Err(error) => outcome = combine_outcomes(outcome, Err(Error::Session(error))),
            }
        }
        for failure in failures {
            let published = failure
                .into_message()
                .map_err(|error| Error::Protocol(ProtocolError::from(error)))
                .and_then(|message| self.publish(message));
            if let Err(error) = published {
                tracing::warn!(%error, "could not report a transcript mirror failure");
            }
        }
        outcome
    }

    fn spawn_callback<Fut>(
        &mut self,
        id: ControlRequestId,
        signal: crate::CancellationToken,
        deadline: Option<Duration>,
        fallback: Result<Option<serde_json::Value>, ProtocolError>,
        future: Fut,
    ) where
        Fut: Future<Output = Result<Option<serde_json::Value>, ProtocolError>> + Send + 'static,
    {
        let callback_id = id.clone();
        let callback_signal = signal.clone();
        let handle = self.callbacks.spawn(async move {
            let response = match deadline {
                None => future.await,
                Some(deadline) => match timeout(deadline, future).await {
                    Ok(response) => response,
                    Err(_) => {
                        callback_signal.cancel();
                        fallback
                    }
                },
            };
            (callback_id, response)
        });
        self.callback_handles.insert(id.clone(), handle);
        self.callback_signals.insert(id, signal);
    }

    /// Writes a callback's answer, if it has one.
    async fn write_callback_response(
        &mut self,
        response: Result<Option<serde_json::Value>, ProtocolError>,
    ) -> Result<(), Error> {
        match response.transpose() {
            Some(frame) => self.write_response(frame).await,
            None => Ok(()),
        }
    }

    async fn write_response(&mut self, frame: Result<serde_json::Value, ProtocolError>) -> Result<(), Error> {
        let frame = frame?;
        if !self.input_open() {
            // The run has settled and input is closed: a late callback answer has nowhere
            // to go, and that must not stop the driver.
            tracing::debug!("dropping a control response written after stdin closed");
            return Ok(());
        }
        self.write(frame).await
    }

    async fn write(&mut self, frame: serde_json::Value) -> Result<(), Error> {
        // Each SDK writes its own JSON spelling: TypeScript `JSON.stringify`, Python `json.dumps`.
        let mut text = match self.argv_style {
            crate::ArgvStyle::TypeScript => crate::js_json::to_string(&frame),
            crate::ArgvStyle::Python => crate::python_json::to_string(&frame),
        }
        .map_err(ProtocolError::from)?;
        text.push('\n');
        if !self.input_open() {
            return Err(Error::NotReadyForWriting);
        }
        let limit = self.limits.write_timeout.map(crate::Timeout::as_duration);
        tokio::select! {
            _ = &mut self.shutdown => { self.shutdown_seen = true; Err(Error::ShutdownRequested) }
            result = async { match limit { Some(limit) => timeout(limit, self.transport.write(&text)).await.map_err(|_| Error::WriteTimeout)?, None => self.transport.write(&text).await } } => result,
        }
    }
}

/// A thrown error becomes the response's `error` text, as `TE.processControlRequest` does with `message`.
fn callback_failure(error: &crate::CallbackError) -> String {
    match error {
        crate::CallbackError::Failed(source) => source.to_string(),
        crate::CallbackError::Cancelled => "Callback cancelled".to_owned(),
    }
}

fn combine_outcomes(primary: Result<(), Error>, cleanup: Result<(), Error>) -> Result<(), Error> {
    match (primary, cleanup) {
        (Ok(()), outcome) | (outcome, Ok(())) => outcome,
        (Err(primary), Err(cleanup)) => Err(Error::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}
