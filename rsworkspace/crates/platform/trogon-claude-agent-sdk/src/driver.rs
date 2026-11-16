use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsStr,
    future::{Future, pending},
    sync::Arc,
    time::Duration,
};

use tokio::{
    sync::{mpsc::UnboundedSender, oneshot},
    task::{AbortHandle, JoinHandle, JoinSet},
    time::{Instant, sleep, sleep_until, timeout},
};

use crate::{
    ClaudeAgentOptions, ControlOperation, ControlRejectionError, ControlRequestId, Error, JsonObject, Limits, Message,
    PermissionDecision, ProtocolError, StderrTail, ToolPermissionHandler, Transport,
    adapters::{EnvironmentVariable, SystemEnvironment},
    constants::{
        self, PYTHON_INITIALIZE_TIMEOUT_SECONDS, PYTHON_INT_LITERAL_REPR_CHARS, TYPESCRIPT_INITIALIZE_TIMEOUT,
    },
    python_text,
    wire::{
        self, ControlResponse, Frame, IncomingControl,
        runtime_payload::{
            EmbeddedMcpPayload, HookMatcherPayload, HooksPayload, InitializeRequest, McpResponsePayload, RequestPayload,
        },
    },
};

#[path = "mcp_manifest.rs"]
mod mcp_manifest;

pub(crate) enum Command {
    /// TypeScript `initialize({first})`: the connect-time request, or `reinitialize()` resending it.
    Initialize { first: bool, response: ControlSender },
    SetMcpServers {
        external: crate::McpServers,
        embedded: crate::SdkMcpServers,
        response: ControlSender,
    },
    Prompt {
        input: Box<crate::UserInput>,
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
        /// The prompts the caller wrote itself, TypeScript `streamInput`'s `count` and Python's `written`, which
        /// count per call. `None` counts every prompt written on the session.
        written: Option<usize>,
        response: oneshot::Sender<Result<(), Error>>,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum Completion {
    Turn,
    Run,
}

/// What the Rust limits count: inputs the CLI still owes a result and tracked tasks. When stdin closes is
/// `RunEnd`'s alone.
#[derive(Default)]
struct RunState {
    input_done: Option<Completion>,
    tasks: HashSet<String>,
    pending_inputs: VecDeque<Option<crate::MessageId>>,
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
                if let Some(count) = message.queued_turn_count {
                    let count = usize::try_from(count).unwrap_or(usize::MAX);
                    if queued_limit.is_some_and(|limit| count > limit) {
                        return Err(Error::TooManyInputs);
                    }
                    while self.pending_inputs.len() > count {
                        self.pending_inputs.pop_front();
                    }
                } else {
                    self.pending_inputs.pop_front();
                }
            }
            Message::System(message) => match message.subtype.as_str() {
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
                    if terminal && let Some(id) = message.data.get("task_id").and_then(serde_json::Value::as_str) {
                        self.tasks.remove(id);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
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

    /// Python `close()` cancels the reader without failing the requests still waiting, so each one runs out its own
    /// timeout. Without a deadline it waits until the caller stops waiting.
    fn time_out_at(self, deadline: Option<Instant>) {
        let operation = self.operation();
        let (Self::Initialize { mut response, .. }
        | Self::Control { mut response, .. }
        | Self::Raw { mut response, .. }) = self;
        tokio::spawn(async move {
            let expired = match deadline {
                Some(deadline) => tokio::select! {
                    () = sleep_until(deadline) => true,
                    () = response.closed() => false,
                },
                None => {
                    response.closed().await;
                    false
                }
            };
            if expired {
                let _ = response.send(Err(Error::ControlTimeout { operation }));
            }
        });
    }
}

type CallbackOutput = (ControlRequestId, Result<Option<Outgoing>, ProtocolError>);

/// A frame the driver writes. The Python argv style answers an `mcp_message` with a frame whose strings may hold
/// lone surrogates, which `serde_json` cannot carry, and writes it as `json.dumps` does.
enum Outgoing {
    Json(serde_json::Value),
    Python(crate::PythonJsonValue),
}

impl From<serde_json::Value> for Outgoing {
    fn from(frame: serde_json::Value) -> Self {
        Self::Json(frame)
    }
}

/// `frame` with `response.response.mcp_response` set to `mcp_response`, as the Python `Query` builds it.
fn python_mcp_frame(frame: serde_json::Value, mcp_response: crate::PythonJsonValue) -> Outgoing {
    let mut frame = crate::PythonJsonValue::from(frame);
    let slot = ["response", "response", "mcp_response"]
        .into_iter()
        .try_fold(&mut frame, |value, key| {
            value.as_object_mut()?.get_mut(&crate::PythonString::from(key))
        });
    if let Some(slot) = slot {
        *slot = mcp_response;
    }
    Outgoing::Python(frame)
}

type McpSessions = indexmap::IndexMap<crate::McpServerName, Arc<dyn crate::SdkMcpSession>>;

/// What an in-process server's transport sent, on its way back to the driver.
pub(crate) enum McpTransportEvent {
    Message { server: String, message: JsonObject },
    Failed { key: String },
}

/// `pendingMcpResponses`: shared with the manifest capture, whose per-server handshakes register waiters too.
#[derive(Clone, Default)]
pub(crate) struct WaiterTable(Arc<std::sync::Mutex<HashMap<String, Waiter>>>);

type Waiter = oneshot::Sender<Result<JsonObject, ()>>;

impl WaiterTable {
    /// Replacing an entry drops the earlier waiter, which is then never answered.
    pub(crate) fn insert(&self, key: String, waiter: oneshot::Sender<Result<JsonObject, ()>>) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, waiter);
    }

    pub(crate) fn remove(&self, key: &str) -> Option<oneshot::Sender<Result<JsonObject, ()>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key)
    }
}

/// The TypeScript `Query` state behind `mcp_message`: `pendingMcpResponses`, keyed by the JavaScript text
/// `${server_name}:${id}`, and the work that feeds it.
pub(crate) struct McpRouting {
    waiters: WaiterTable,
    tasks: JoinSet<()>,
    outbound: tokio::sync::mpsc::UnboundedSender<McpTransportEvent>,
    inbound: tokio::sync::mpsc::UnboundedReceiver<McpTransportEvent>,
    /// `initializeHasBeenWritten`: set once the first `initialize` request has been written.
    initialize_written: bool,
    /// `afterInitialize`: forwarded frames waiting for the `initialize` request, in order.
    held: std::collections::VecDeque<serde_json::Value>,
    /// `sdkMcpToolsChangedDuringCapture`: servers that announced a tool list change before `initialize`.
    tools_changed: std::collections::BTreeSet<String>,
}

impl McpRouting {
    /// Forwards what a session's transport sends on its own: notifications, server requests and late replies.
    fn forward(&mut self, name: &crate::McpServerName, session: &Arc<dyn crate::SdkMcpSession>) {
        if let Some(mut unsolicited) = session.take_outbound() {
            let outbound = self.outbound.clone();
            let server = name.as_str().to_owned();
            self.tasks.spawn(async move {
                while let Some(message) = unsolicited.recv().await {
                    let event = McpTransportEvent::Message {
                        server: server.clone(),
                        message,
                    };
                    if outbound.send(event).is_err() {
                        break;
                    }
                }
            });
        }
    }

    pub(crate) fn new() -> Self {
        let (outbound, inbound) = tokio::sync::mpsc::unbounded_channel();
        Self {
            waiters: WaiterTable::default(),
            tasks: JoinSet::new(),
            outbound,
            inbound,
            initialize_written: false,
            held: std::collections::VecDeque::new(),
            tools_changed: std::collections::BTreeSet::new(),
        }
    }
}

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
    /// TypeScript `abortController`: once cancelled, the process is stopped and the run ends with an `AbortError`.
    abort: crate::CancellationToken,
    limits: Limits,
    handler: Option<Arc<dyn ToolPermissionHandler>>,
    pending: HashMap<ControlRequestId, PendingControl>,
    callbacks: JoinSet<CallbackOutput>,
    callback_handles: HashMap<ControlRequestId, AbortHandle>,
    callback_signals: HashMap<ControlRequestId, crate::CancellationToken>,
    /// Python: callbacks that lost their request id to a duplicate still run and answer, but a cancel no longer
    /// reaches them.
    detached_callbacks: HashMap<tokio::task::Id, crate::CancellationToken>,
    /// TypeScript: callbacks whose request was cancelled. Their signal fired and their request id is free again,
    /// but they still run and their answer is still written.
    cancelled_callbacks: HashSet<tokio::task::Id>,
    hook_callbacks: HashMap<String, HookCallback>,
    sdk_mcp_servers: McpSessions,
    mcp: McpRouting,
    /// `captureSdkMcpManifests`, in the TypeScript dialect only: Python has no manifest capture.
    capture_manifests: bool,
    initialize_request: serde_json::Value,
    initialize_expectations: InitializeExpectations,
    initialize_timeout: Option<Duration>,
    elicitation_handler: Option<Arc<dyn crate::ElicitationHandler>>,
    user_dialog_handler: Option<Arc<dyn crate::UserDialogHandler>>,
    oauth_token_handler: Option<Arc<dyn crate::OAuthTokenHandler>>,
    host_auth_token_handler: Option<Arc<dyn crate::HostAuthTokenHandler>>,
    /// The work secret answerer and `uiHost` that `ClientHandle` installs.
    host: crate::remote_control::HostState,
    session: Option<crate::SessionRuntime<F>>,
    mirror_flush: Option<MirrorBarrier>,
    deferred_messages: VecDeque<(Message, Option<MirrorBarrier>)>,
    /// Frames the run-end machine has not seen yet. Both upstream readers wait for the transcript flush before they
    /// handle a result, so the machine sees a result, and every frame after it, only once that flush finished. The flag
    /// marks a result that starts a flush.
    held_observations: VecDeque<(Message, bool)>,
    /// TypeScript `promptHold`: while the first initialize is unanswered and a blocking hook or managed settings
    /// need its confirmation, prompts and most controls wait behind it.
    initialize_hold: bool,
    /// What the handles see of `initialize_hold`.
    prompt_hold: PromptHold,
    held_commands: VecDeque<Command>,
    next_id: u64,
    /// The `mcp_message` ids of the line just read, until the Python `mcp_message` handler takes them.
    wire_ids: Option<crate::McpWireIds>,
    state: State,
    run_state: RunState,
    run_end: crate::run_end::RunEnd,
    argv_style: crate::ArgvStyle,
    message_decoding: crate::MessageDecoding,
    js_runtime: crate::JsRuntime,
    /// The last `is_error` result, kept while nothing else has arrived since, to turn the exit error into a
    /// result error the way both upstream SDKs do.
    last_error_result: Option<JsonObject>,
    /// The CLI path until the first `system`/`init` message has been checked, then `None`.
    version_check: Option<String>,
    latest_commands: LatestCommands,
    /// See [`SpawnOptions::startup_tap`].
    startup_tap: Option<UnboundedSender<crate::startup_timeout::StartupEvent>>,
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

/// TypeScript `promptHold` as `Query.streamInput` reads it, shared with the handles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum HoldState {
    /// `promptHold === undefined`: no hold applies, or the first initialize was confirmed.
    Clear = 0,
    /// The first initialize is unanswered and what is written waits behind it.
    Pending = 1,
    /// The first initialize failed: the hold stays set and resolves `false`.
    Failed = 2,
}

/// See [`HoldState`]. The driver sets it; `stream_input` reads it.
#[derive(Clone, Default)]
pub(crate) struct PromptHold(Arc<std::sync::atomic::AtomicU8>);

impl PromptHold {
    pub(crate) fn state(&self) -> HoldState {
        match self.0.load(std::sync::atomic::Ordering::Acquire) {
            1 => HoldState::Pending,
            2 => HoldState::Failed,
            _ => HoldState::Clear,
        }
    }

    fn set(&self, state: HoldState) {
        self.0.store(state as u8, std::sync::atomic::Ordering::Release);
    }
}

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
                .map(|(name, definition)| {
                    definition
                        .to_wire_json_for(options.argv_style)
                        .map(|value| (name.as_str().to_owned(), value))
                })
                .collect::<Result<JsonObject, _>>()
                .map_err(ProtocolError::from)?,
        )
    };
    // TypeScript sends a `hooks` object and a `supportedDialogKinds` list whenever the caller gave one, even empty.
    let send_empty = |when_empty: crate::EmptyCollection| typescript && when_empty == crate::EmptyCollection::Sent;
    let supported_dialog_kinds = (!options.supported_dialog_kinds.is_empty()
        || send_empty(options.supported_dialog_kinds_when_empty))
    .then(|| {
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
        hooks: (!options.hooks.is_empty() || send_empty(options.hooks_when_empty)).then_some(hooks),
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
    pub abort: crate::CancellationToken,
    pub terminal: oneshot::Sender<Result<(), Error>>,
    pub transport: T,
    pub initialization: Initialization,
    /// The CLI path to check against the minimum version, for a transport that runs the CLI itself.
    pub version_check: Option<String>,
    pub latest_commands: LatestCommands,
    pub prompt_hold: PromptHold,
    pub host: crate::remote_control::HostState,
    /// `startup()`: the hook and startup-mark frames, for the initialize timer. Closing the receiver ends the
    /// observation.
    pub startup_tap: Option<UnboundedSender<crate::startup_timeout::StartupEvent>>,
    /// Where the response to the connect-time initialize request goes. The driver writes that request before it
    /// handles anything else.
    pub first_initialize: ControlSender,
}

/// The initialize request and the callbacks it registers, built before any transport exists so that a bad
/// configuration fails before a process is spawned.
pub(crate) struct Initialization {
    request: serde_json::Value,
    hook_callbacks: HashMap<String, HookCallback>,
    expectations: InitializeExpectations,
    timeout: Option<Duration>,
    /// The run-end ceiling, or `None` when the environment asks for no limit.
    run_end: Option<Duration>,
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
            run_end: crate::run_end_ceiling::resolve(options, environment),
        })
    }
}

impl Initialization {
    /// Sends `supportedDialogKinds: []` when no kind was configured, as the browser query does for an empty list.
    pub(crate) fn declare_dialog_kinds(&mut self) {
        if let Some(request) = self.request.as_object_mut() {
            request
                .entry("supportedDialogKinds")
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        }
    }
}

/// Python: `max(int(os.environ.get("CLAUDE_CODE_STREAM_CLOSE_TIMEOUT", "60000")) / 1000.0, 60.0)` seconds, handed
/// to `anyio.fail_after`. A wait too long for `Duration` never ends, as Python's does not in practice.
pub(crate) fn python_initialize_timeout(value: Option<std::ffi::OsString>) -> Result<Duration, Error> {
    let seconds = python_initialize_seconds(value.as_deref())?;
    Ok(Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX))
}

/// The float of seconds Python computes, with the error each step of that expression raises.
pub(crate) fn python_initialize_seconds(value: Option<&std::ffi::OsStr>) -> Result<f64, Error> {
    let Some(value) = value else {
        return Ok(PYTHON_INITIALIZE_TIMEOUT_SECONDS);
    };
    let text = python_text::from_os_str(value);
    let milliseconds = match python_text::int(&text) {
        Ok(milliseconds) => milliseconds.to_f64().ok_or(Error::StreamCloseTimeoutOverflow)?,
        Err(python_text::IntError::InvalidLiteral) => {
            return Err(Error::InvalidStreamCloseTimeout {
                literal: python_text::repr(&text)
                    .chars()
                    .take(PYTHON_INT_LITERAL_REPR_CHARS)
                    .collect(),
            });
        }
        Err(python_text::IntError::TooManyDigits(digits)) => return Err(Error::StreamCloseTimeoutDigits { digits }),
    };
    let seconds = milliseconds / 1000.0;
    Ok(if PYTHON_INITIALIZE_TIMEOUT_SECONDS > seconds {
        PYTHON_INITIALIZE_TIMEOUT_SECONDS
    } else {
        seconds
    })
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
        abort,
        terminal,
        mut transport,
        initialization,
        version_check,
        latest_commands,
        prompt_hold,
        host,
        startup_tap,
        first_initialize,
    } = input;
    transport.connect().await?;
    let capture_manifests = options.argv_style == crate::ArgvStyle::TypeScript
        && options.captures_sdk_mcp_manifests(
            EnvironmentVariable::var_os(&SystemEnvironment, OsStr::new("CLAUDE_AGENT_SDK_DISABLE_MCP_MANIFESTS"))
                .as_deref(),
        );
    let initialize_hold = options.argv_style == crate::ArgvStyle::TypeScript
        && (initialization.expectations.blocking_hooks || initialization.expectations.managed_settings);
    if initialize_hold {
        prompt_hold.set(HoldState::Pending);
    }
    let driver = Driver {
        transport,
        input_ended: false,
        commands,
        messages,
        overflow: VecDeque::new(),
        shutdown,
        shutdown_seen: false,
        abort,
        limits: options.limits,
        handler: options.permission_handler,
        pending: HashMap::new(),
        callbacks: JoinSet::new(),
        callback_handles: HashMap::new(),
        callback_signals: HashMap::new(),
        detached_callbacks: HashMap::new(),
        cancelled_callbacks: HashSet::new(),
        hook_callbacks: initialization.hook_callbacks,
        sdk_mcp_servers: options
            .sdk_mcp_servers
            .iter()
            .map(|(name, server)| (name.clone(), server.open_session()))
            .collect(),
        mcp: McpRouting::new(),
        capture_manifests,
        initialize_request: initialization.request,
        initialize_expectations: initialization.expectations,
        initialize_timeout: initialization.timeout,
        elicitation_handler: options.elicitation_handler,
        user_dialog_handler: options.user_dialog_handler,
        oauth_token_handler: options.oauth_token_handler,
        host_auth_token_handler: options.host_auth_token_handler,
        host,
        session,
        mirror_flush: None,
        deferred_messages: VecDeque::new(),
        held_observations: VecDeque::new(),
        initialize_hold,
        prompt_hold,
        held_commands: VecDeque::new(),
        next_id: 0,
        wire_ids: None,
        state: State::Initializing,
        run_state: RunState::default(),
        run_end: crate::run_end::RunEnd::new(options.argv_style, initialization.run_end),
        argv_style: options.argv_style,
        message_decoding: options.message_decoding,
        js_runtime: options.js_runtime,
        last_error_result: None,
        version_check,
        latest_commands,
        startup_tap,
        verbatim_prompts: options.verbatim_prompts == crate::VerbatimPrompts::Enabled,
        warn_unsupported_verbatim: options.verbatim_prompts == crate::VerbatimPrompts::Enabled
            && options.argv_style == crate::ArgvStyle::Python,
    };
    let mut driver = driver;
    // Only the TypeScript `Query` forwards what a server's transport sends on its own.
    if driver.argv_style == crate::ArgvStyle::TypeScript {
        for (name, session) in &driver.sdk_mcp_servers {
            driver.mcp.forward(name, session);
        }
    }
    // `sdk.mjs` writes the initialize request while it constructs the Query, so it is on stdin before the caller
    // holds anything; the outcome travels into the task so a failed write ends the run like any other error.
    let started = driver.start(first_initialize).await;
    Ok(tokio::spawn(driver.run(started, terminal)))
}

impl<T, F> Driver<T, F>
where
    T: Transport,
    F: crate::adapters::SessionFileSystem,
{
    async fn run(mut self, started: Result<(), Error>, terminal: oneshot::Sender<Result<(), Error>>) {
        let primary = match started {
            Ok(()) => Box::pin(self.event_loop()).await,
            Err(error) => Err(error),
        };
        let primary = match primary {
            Err(Error::ShutdownRequested) => Ok(()),
            outcome => outcome,
        };
        // Kept unanswered until the run ends, as they were while they waited in the queue.
        let _unapplied = if self.shutdown_seen && primary.is_ok() {
            Box::pin(self.write_queued_prompts()).await
        } else {
            Vec::new()
        };
        self.callbacks.abort_all();
        self.mcp.tasks.abort_all();
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
        let mirror_cleanup = Box::pin(self.finalize_mirror()).await;
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
            match (self.shutdown_seen, self.argv_style) {
                // TypeScript `close()` rejects what is still unanswered with "Query closed before response received".
                (true, crate::ArgvStyle::TypeScript) => pending.response.send_error(Error::QueryClosed),
                (true, crate::ArgvStyle::Python) => pending.response.time_out_at(pending.deadline),
                (false, _) => pending.response.send_error(Error::DriverStopped),
            }
        }
        self.commands.close();
        self.set_error_result(None);
        drop(self.messages);
        let _ = terminal.send(outcome);
    }

    /// `sdk.mjs` writes a prompt to stdin as `query()` takes it, so one taken before `close()` is on stdin when the
    /// input ends. The close can reach the driver ahead of a prompt queued before it, so those prompts are written
    /// now; the other commands are handed back untouched.
    async fn write_queued_prompts(&mut self) -> Vec<Command> {
        let mut unapplied = Vec::new();
        while let Some(command) = self.commands.try_recv() {
            if !matches!(command, Command::Prompt { .. }) {
                unapplied.push(command);
                continue;
            }
            if let Err(error) = Box::pin(self.command(command)).await {
                tracing::debug!(%error, "a prompt queued before the close could not be written");
                break;
            }
        }
        unapplied
    }

    /// Writes the connect-time initialize request. `sdk.mjs` writes it while it constructs the Query, before the
    /// caller holds anything to close, and Python waits for its answer; either way a close that follows at once
    /// finds the request already on stdin. A process spawned under an aborted signal writes nothing.
    async fn start(&mut self, first_initialize: ControlSender) -> Result<(), Error> {
        if self.abort.is_cancelled() {
            return Err(crate::AbortError::PROCESS.into());
        }
        Box::pin(self.command(Command::Initialize {
            first: true,
            response: first_initialize,
        }))
        .await
    }

    /// Whether another document may be written: the transport accepts writes and input has not been ended.
    fn input_open(&self) -> bool {
        !self.input_ended && self.transport.accepts_writes()
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
    /// `MessageParseError` carrying the raw object. The object drops at any nesting.
    fn decode_message(&self, mut object: JsonObject) -> Result<Frame, Error> {
        match wire::parse_value(&object, self.message_decoding) {
            Ok(frame) => {
                crate::deep_json::clear_object(&mut object);
                Ok(frame)
            }
            Err(error) => Err(crate::MessageParseError::new(error, Some(object)).into()),
        }
    }

    /// Replaces the pending error result, dropping the one it held at any nesting.
    fn set_error_result(&mut self, data: Option<JsonObject>) {
        if let Some(mut replaced) = std::mem::replace(&mut self.last_error_result, data) {
            crate::deep_json::clear_object(&mut replaced);
        }
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
            // A taken or dropped signal leaves the receiver spent, and polling it again panics.
            if self.shutdown_seen || !matches!(self.shutdown.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                self.shutdown_seen = true;
                return Ok(());
            }
            if self.abort.is_cancelled() {
                return Err(crate::AbortError::PROCESS.into());
            }
            if let Some(error) = self.expired_control() {
                return Err(error);
            }
            if self.run_end.close_due() && !self.holds_input_for_callbacks() {
                self.run_end.closing_input();
                self.end_input().await;
            }
            let deadline = self.pending.values().filter_map(|request| request.deadline).min();
            let run_deadline = self.run_end.deadline();
            // Python stops reading stdout while its message buffer is full, so nothing else on stdout is handled
            // until the caller takes a message.
            let reading = self.overflow.is_empty() && !self.deferred_full();
            tokio::select! {
                _ = &mut self.shutdown, if !self.shutdown_seen => { self.shutdown_seen = true; return Ok(()); }
                () = self.abort.cancelled() => return Err(crate::AbortError::PROCESS.into()),
                ready = self.messages.ready(), if !self.overflow.is_empty() => {
                    if !ready { return Err(Error::DriverStopped); }
                    self.drain_overflow()?;
                }
                message = self.transport.read_message(), if reading => {
                    let Some(message) = message? else {
                        self.finish_mirror().await?;
                        self.run_end.reader_gone();
                        // A clean exit in the middle of a turn ends the messages in both SDKs; only a failed exit
                        // is an error, and the process close reports it.
                        let unfinished = match self.state {
                            State::Initializing => true,
                            State::Running => self.limits.end_without_result == crate::EndWithoutResult::Fail,
                            State::Idle => false,
                        };
                        return if unfinished || !self.pending.is_empty() { Err(Error::UnexpectedEof) } else { Ok(()) };
                    };
                    self.wire_ids = self.transport.take_wire_ids();
                    self.tap_startup(&message);
                    let parsed = self.decode_message(message)?;
                    Box::pin(self.frame(parsed)).await?;
                }
                flushed = async { match self.mirror_flush.as_mut() { Some(flush) => flush.await, None => pending().await } }, if self.mirror_flush.is_some() => {
                    self.mirror_flush = None;
                    self.release_observations();
                    let failures = flushed?;
                    for failure in failures { self.publish(failure.into_message().map_err(ProtocolError::from)?)?; }
                    self.publish_deferred()?;
                }
                outbound = self.mcp.inbound.recv() => {
                    if let Some(event) = outbound {
                        Box::pin(self.mcp_transport_event(event)).await?;
                    }
                }
                _ = self.mcp.tasks.join_next(), if !self.mcp.tasks.is_empty() => {}
                command = self.commands.recv() => {
                    let Some(command) = command else { return Ok(()); };
                    Box::pin(self.command(command)).await?;
                }
                callback = self.callbacks.join_next_with_id(), if !self.callbacks.is_empty() => {
                    match callback {
                        Some(Ok((task_id, (id, response)))) if self.callback_handles.get(&id).is_some_and(|handle| handle.id() == task_id) => {
                            self.callback_handles.remove(&id);
                            self.callback_signals.remove(&id);
                            self.write_callback_response(response).await?;
                        }
                        Some(Ok((task_id, (_, response)))) if self.cancelled_callbacks.remove(&task_id) => {
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
                        // An aborted task, or the callback of a request cancelled from the session, ends quietly.
                        Some(Err(error)) if !error.is_cancelled() && !self.cancelled_callbacks.remove(&error.id()) && (self.detached_callbacks.contains_key(&error.id()) || self.callback_handles.values().any(|handle| handle.id() == error.id())) => return Err(Error::CallbackTask(error)),
                        _ => {}
                    }
                }
                _ = async { match deadline { Some(deadline) => sleep_until(deadline).await, None => pending::<()>().await } } => {
                    if let Some(error) = self.expired_control() { return Err(error); }
                }
                _ = async { match run_deadline { Some(deadline) => sleep_until(deadline).await, None => pending::<()>().await } } => {
                    self.run_end.ceiling_elapsed();
                }
            }
        }
    }

    /// `hasBidirectionalNeeds()`. TypeScript: SDK MCP servers, hooks, a permission, elicitation or user dialog
    /// handler, an auth callback or a work secret answerer. Python's `_has_bidirectional_needs` knows only SDK MCP
    /// servers, hooks and the permission callback, because it has no other callback.
    fn bidirectional_needs(&self) -> bool {
        let hooks = self
            .initialize_request
            .get("hooks")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|hooks| !hooks.is_empty());
        let common = !self.sdk_mcp_servers.is_empty() || hooks || self.handler.is_some();
        match self.argv_style {
            crate::ArgvStyle::Python => common,
            crate::ArgvStyle::TypeScript => {
                common
                    || self.elicitation_handler.is_some()
                    || self.user_dialog_handler.is_some()
                    || self.oauth_token_handler.is_some()
                    || self.host_auth_token_handler.is_some()
                    || self.host.answerer().is_some()
            }
        }
    }

    /// Hands the frames the startup timer follows to `startup()` while it still waits for initialize.
    fn tap_startup(&mut self, frame: &JsonObject) {
        let Some(tap) = &self.startup_tap else {
            return;
        };
        if tap.is_closed() {
            self.startup_tap = None;
        } else if let Some(event) = crate::startup_timeout::StartupEvent::from_frame(frame, Instant::now()) {
            let _ = tap.send(event);
        }
    }

    /// `sdk.mjs` ends its input only once the callbacks it started have answered; Python closes at the result
    /// whatever is still running. A callback that starts after stdin closed has nowhere to answer.
    fn holds_input_for_callbacks(&self) -> bool {
        self.argv_style == crate::ArgvStyle::TypeScript
            && !self.input_ended
            && (!self.callback_handles.is_empty() || !self.detached_callbacks.is_empty())
    }

    fn running_callbacks(&self) -> usize {
        self.callback_handles.len() + self.detached_callbacks.len() + self.cancelled_callbacks.len()
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
        let first_initialize = matches!(request.response, ControlResponder::Initialize { first: true, .. });
        request.response.send_error(error());
        // The timer calls `cleanup(error)`, which ends the run.
        if first_initialize && self.initialize_hold {
            self.abandon_initialize(&std::sync::Arc::new(error()));
        }
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
        let id = match self.argv_style {
            crate::ArgvStyle::TypeScript => ControlRequestId::typescript(),
            crate::ArgvStyle::Python => ControlRequestId::python(self.next_id),
        };
        // TypeScript `request`: `this.transport.expectControlResponse?.(id)` before the request is written.
        self.transport.expect_control_response(id.as_str());
        Ok(id)
    }
    fn controls_full(&self) -> bool {
        // The initialize handshake is not an application control, so it does not use up the limit.
        self.limits.max_pending_controls.is_some_and(|limit| {
            self.pending
                .values()
                .filter(|pending| !matches!(pending.response, ControlResponder::Initialize { .. }))
                .count()
                >= limit.get()
        })
    }

    #[cfg(test)]
    async fn admit_callback(&mut self, id: &ControlRequestId) -> Result<bool, Error> {
        Ok(self.admit_request(id)? && self.admit_capacity(id).await?)
    }

    /// Applies the duplicate policy to a request whose id is still in flight.
    fn admit_request(&mut self, id: &ControlRequestId) -> Result<bool, Error> {
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
        Ok(true)
    }

    /// Applies the opt-in limit on running callbacks before a handler starts; requests answered at once never
    /// count against it.
    async fn admit_capacity(&mut self, id: &ControlRequestId) -> Result<bool, Error> {
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

    /// Answers one admitted inbound control request the way the dialect's SDK does.
    async fn control_request(&mut self, id: ControlRequestId, request: IncomingControl) -> Result<(), Error> {
        let style = self.argv_style;
        let python = style == crate::ArgvStyle::Python;
        match request {
            IncomingControl::Unsupported { subtype } => {
                if let Some(response) = wire::unsupported_response(&id, &subtype, style) {
                    self.write_response(response).await?;
                }
            }
            IncomingControl::Permission(mut request) => {
                // `claude_agent_sdk` reads `input`, then checks for a callback, then rebuilds the suggestions,
                // then reads `tool_name`, answering the first `KeyError` or `from_dict` failure with its message.
                let input = request.raw_request.get("input").map(crate::deep_json::clone_value);
                if python && input.is_none() {
                    return self.write_response(wire::callback_error_response(&id, "'input'")).await;
                }
                let Some(handler) = self.handler.clone() else {
                    return self
                        .write_response(wire::missing_permission_handler_response(&id, style))
                        .await;
                };
                if python {
                    match crate::permission_suggestions::python(request.raw_request.get("permission_suggestions")) {
                        Ok(suggestions) => request.suggestions = suggestions,
                        Err(message) => return self.write_response(wire::callback_error_response(&id, &message)).await,
                    }
                    if !request.raw_request.contains_key("tool_name") {
                        return self
                            .write_response(wire::callback_error_response(&id, "'tool_name'"))
                            .await;
                    }
                }
                if !self.admit_capacity(&id).await? {
                    return Ok(());
                }
                let input = input.unwrap_or(serde_json::Value::Null);
                let tool_use_id = request.tool_use_id.clone();
                let signal = request.signal.clone();
                let callback_id = id.clone();
                let fallback = wire::permission_response(
                    &id,
                    crate::deep_json::clone_value(&input),
                    tool_use_id.as_ref(),
                    PermissionDecision::deny("Permission callback timed out"),
                    style,
                );
                self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                    let decision = handler.decide(*request).await;
                    wire::permission_response(&callback_id, input, tool_use_id.as_ref(), decision, style)
                });
            }
            IncomingControl::Hook {
                callback_id,
                input,
                tool_use_id,
            } => {
                let Some(callback) = self.hook_callbacks.get(&callback_id).cloned() else {
                    return self
                        .write_response(wire::unknown_hook_response(&id, &callback_id))
                        .await;
                };
                if !self.admit_capacity(&id).await? {
                    return Ok(());
                }
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
                        Err(error) => wire::handler_error_response(&response_id, &error, style),
                    }
                });
            }
            IncomingControl::Mcp { server_name, message } => match style {
                crate::ArgvStyle::TypeScript => self.typescript_mcp_message(id, server_name, message).await?,
                crate::ArgvStyle::Python => self.python_mcp_message(id, server_name, message).await?,
            },
            IncomingControl::Elicitation(request) => {
                let Some(handler) = self.elicitation_handler.clone() else {
                    // TypeScript declines; Python does not handle the subtype at all.
                    let response = if python {
                        wire::callback_error_response(&id, "Unsupported control request subtype: elicitation")
                    } else {
                        wire::success_response(
                            &id,
                            crate::ElicitationResponse::Decline
                                .to_wire_json()
                                .map_err(ProtocolError::from)?,
                        )
                    };
                    return self.write_response(response).await;
                };
                if !self.admit_capacity(&id).await? {
                    return Ok(());
                }
                let signal = crate::CancellationToken::new();
                let request = crate::ElicitationRequest {
                    server_name: crate::McpServerName::from(request.server_name),
                    message: request.message,
                    mode: request.mode.map(crate::ElicitationMode::from),
                    url: request.url,
                    elicitation_id: request.elicitation_id,
                    requested_schema: request.requested_schema,
                    title: request.title,
                    display_name: request.display_name,
                    description: request.description,
                    request_id: id.clone(),
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
                    match handler.call(request).await {
                        Ok(crate::ElicitationResponse::AnsweredOutOfBand) => Ok(None),
                        Ok(response) => wire::success_response(&response_id, response.to_wire_json()?).map(Some),
                        Err(error) => wire::handler_error_response(&response_id, &error, style),
                    }
                });
            }
            IncomingControl::UserDialog {
                kind,
                payload,
                tool_use_id,
            } => {
                let Some(handler) = self.user_dialog_handler.clone() else {
                    // TypeScript stays silent so a capable client, or the worker's deadline, settles the dialog;
                    // Python does not handle the subtype at all.
                    if python && let Some(response) = wire::unsupported_response(&id, "request_user_dialog", style) {
                        self.write_response(response).await?;
                    }
                    return Ok(());
                };
                if !self.admit_capacity(&id).await? {
                    return Ok(());
                }
                let signal = crate::CancellationToken::new();
                let request = crate::UserDialogRequest {
                    dialog_kind: crate::DialogKind::from(kind),
                    payload,
                    tool_use_id,
                    request_id: id.clone(),
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
                    match handler.call(request).await {
                        Ok(crate::UserDialogResponse::AnsweredOutOfBand) => Ok(None),
                        Ok(response) => wire::success_response(&response_id, response.to_wire_json()?).map(Some),
                        Err(error) => wire::handler_error_response(&response_id, &error, style),
                    }
                });
            }
            IncomingControl::OAuthTokenRefresh { kind, token_state } => {
                // Python has no token callbacks and answers the subtype as unsupported.
                let handler = self.oauth_token_handler.clone().filter(|_| !python);
                let Some(handler) = handler else {
                    if let Some(response) = wire::unsupported_response(&id, "oauth_token_refresh", style) {
                        self.write_response(response).await?;
                    }
                    return Ok(());
                };
                if !self.admit_capacity(&id).await? {
                    return Ok(());
                }
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
                        Err(error) => wire::handler_error_response(&response_id, &error, style),
                    }
                });
            }
            IncomingControl::HostAuthTokenRefresh { status } => {
                let handler = self.host_auth_token_handler.clone().filter(|_| !python);
                let Some(handler) = handler else {
                    if let Some(response) = wire::unsupported_response(&id, "host_auth_token_refresh", style) {
                        self.write_response(response).await?;
                    }
                    return Ok(());
                };
                if !self.admit_capacity(&id).await? {
                    return Ok(());
                }
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
                        Err(error) => wire::handler_error_response(&response_id, &error, style),
                    }
                });
            }
            IncomingControl::RemoteControlWork { session_id } => {
                let Some(answerer) = self.host.answerer().filter(|_| !python) else {
                    if let Some(response) = wire::unsupported_response(&id, "remote_control_work_secret", style) {
                        self.write_response(response).await?;
                    }
                    return Ok(());
                };
                match crate::remote_control::same_requested_session(&answerer.session_id, session_id.as_ref()) {
                    Err(message) => return self.write_response(wire::callback_error_response(&id, &message)).await,
                    Ok(false) => {
                        return self
                            .write_response(wire::callback_error_response(
                                &id,
                                "remote_control_work_secret names a session this host did not attach.",
                            ))
                            .await;
                    }
                    Ok(true) => {}
                }
                if !self.admit_capacity(&id).await? {
                    return Ok(());
                }
                let signal = crate::CancellationToken::new();
                let request = crate::WorkSecretRefreshRequest {
                    session_id: answerer.session_id.clone(),
                    signal: signal.clone(),
                };
                let response_id = id.clone();
                let fallback = wire::callback_error_response(&id, "refreshWorkSecret callback timed out").map(Some);
                self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                    match answerer.refresh.call(request).await {
                        Ok(secret) => {
                            let secret = secret.filter(|secret| !secret.is_empty());
                            let mut response = JsonObject::new();
                            response.insert("work_secret".into(), secret.map_or(serde_json::Value::Null, Into::into));
                            wire::success_response(&response_id, response).map(Some)
                        }
                        Err(error) => wire::handler_error_response(&response_id, &error, style),
                    }
                });
            }
            IncomingControl::Ui { subtype, payload } => {
                let handler = self
                    .host
                    .ui_host()
                    .filter(|_| !python)
                    .and_then(|host| host.handler(&subtype));
                let Some(handler) = handler else {
                    if let Some(response) = wire::unsupported_response(&id, &subtype, style) {
                        self.write_response(response).await?;
                    }
                    return Ok(());
                };
                if !self.admit_capacity(&id).await? {
                    return Ok(());
                }
                let signal = crate::CancellationToken::new();
                let request = crate::UiRequest {
                    payload,
                    signal: signal.clone(),
                };
                let response_id = id.clone();
                let fallback = wire::callback_error_response(&id, "uiHost callback timed out").map(Some);
                self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
                    match handler.call(request).await {
                        Ok(Some(response)) => wire::success_response(&response_id, response).map(Some),
                        Ok(None) => wire::success_without_response(&response_id).map(Some),
                        Err(error) => wire::handler_error_response(&response_id, &error, style),
                    }
                });
            }
        }
        Ok(())
    }

    /// `sdk.mjs` `writeBehindPromptHold`: what waits for the first initialize to be confirmed. A prompt always
    /// does, `interrupt` and `cancel_async_message` do, and with managed settings every request but `initialize`
    /// and `get_binary_version` does. Ending the input waits behind a held prompt, since the stream writes in
    /// order.
    fn held_by_initialize(&self, command: &Command) -> bool {
        if !self.initialize_hold {
            return false;
        }
        let managed = self.initialize_expectations.managed_settings;
        match command {
            Command::Prompt { .. } => true,
            Command::Control { request, .. } | Command::RawControl { request, .. } => {
                let subtype = request.get("subtype").and_then(serde_json::Value::as_str);
                matches!(subtype, Some("interrupt" | "cancel_async_message"))
                    || (managed && !matches!(subtype, Some("initialize" | "get_binary_version")))
            }
            Command::SetMcpServers { .. } => managed,
            Command::FinishInput { .. } => !self.held_commands.is_empty(),
            Command::Initialize { .. } => false,
        }
    }

    /// The first initialize was confirmed: what waited behind it goes out in order.
    async fn release_held_commands(&mut self) -> Result<(), Error> {
        if self.initialize_hold {
            self.prompt_hold.set(HoldState::Clear);
        }
        self.initialize_hold = false;
        while let Some(command) = self.held_commands.pop_front() {
            self.command(command).await?;
        }
        Ok(())
    }

    /// The first initialize failed. `sdk.mjs` `cleanup(error)` rejects every request still unanswered with that
    /// error, and what waited behind the hold is never written: a prompt resolves quietly, the way the hold
    /// resolves `false`, and a held request rejects with the same error.
    fn abandon_initialize(&mut self, error: &std::sync::Arc<Error>) {
        if self.initialize_hold {
            self.prompt_hold.set(HoldState::Failed);
        }
        self.initialize_hold = false;
        for command in self.held_commands.drain(..) {
            match command {
                Command::Prompt { response, .. } | Command::FinishInput { response, .. } => {
                    let _ = response.send(Ok(()));
                }
                Command::Control { response, .. }
                | Command::RawControl { response, .. }
                | Command::SetMcpServers { response, .. }
                | Command::Initialize { response, .. } => {
                    let _ = response.send(Err(Error::InitializationFailed(error.clone())));
                }
            }
        }
        for (_, pending) in self.pending.drain() {
            pending.response.send_error(Error::InitializationFailed(error.clone()));
        }
    }

    async fn command(&mut self, command: Command) -> Result<(), Error> {
        if self.held_by_initialize(&command) {
            self.held_commands.push_back(command);
            return Ok(());
        }
        match command {
            Command::Initialize { first, response } => {
                let mut request = self.initialize_request.clone();
                // TypeScript captures SDK MCP server manifests for the first initialize only:
                // `e && captureSdkMcpManifests === true && sdkMcpServers.size > 0`.
                if !first && let Some(object) = request.as_object_mut() {
                    object.remove("sdkMcpServerManifests");
                }
                if first
                    && self.capture_manifests
                    && !self.sdk_mcp_servers.is_empty()
                    && let Some(manifests) = self.capture_sdk_mcp_manifests().await?
                {
                    crate::mcp_routing::insert_manifests(&mut request, manifests);
                }
                crate::mcp_routing::trim_changed_manifests(&mut request, &self.mcp.tools_changed);
                self.mcp.tools_changed.clear();
                let started = self
                    .start_control(ControlResponder::Initialize { first, response }, request)
                    .await;
                // `finally { markInitializeWritten() }`: forwarded frames go out behind the request, in order.
                self.mcp.initialize_written = true;
                started?;
                while let Some(frame) = self.mcp.held.pop_front() {
                    self.write_response(Ok(frame)).await?;
                }
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
                    let session = match self.sdk_mcp_servers.get(&name).cloned() {
                        Some(session) => session,
                        None => {
                            let session = registry.open_session();
                            if self.argv_style == crate::ArgvStyle::TypeScript {
                                self.mcp.forward(&name, &session);
                            }
                            session
                        }
                    };
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
                    // TypeScript writes a prompt right behind the initialize request; only Python waits for the
                    // response before it lets one through.
                    State::Initializing if self.argv_style == crate::ArgvStyle::Python => {
                        let _ = response.send(Err(Error::NotInitialized));
                        return Ok(());
                    }
                    State::Initializing | State::Idle | State::Running => {}
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
                // A message object written as given carries its own uuid, so read it from the frame.
                let uuid = frame
                    .get("uuid")
                    .and_then(serde_json::Value::as_str)
                    .map(crate::MessageId::from);
                self.write(frame).await?;
                self.state = State::Running;
                self.run_end.prompt_written();
                self.run_state.pending_inputs.push_back(uuid);
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
            Command::FinishInput {
                completion,
                written,
                response,
            } => {
                self.run_state.input_done = Some(completion);
                let needs = self.bidirectional_needs();
                self.run_end
                    .finish(matches!(completion, Completion::Turn), needs, written);
                let _ = response.send(Ok(()));
            }
        }
        Ok(())
    }

    /// Applies the initialize checks of `sdk.mjs` `Query.initialize({first})`, in its order. A refusal ends the
    /// session, after a `reinitialize()` too.
    async fn finish_initialize(
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
            if self.argv_style == crate::ArgvStyle::TypeScript {
                self.abandon_initialize(&std::sync::Arc::new(error()));
            }
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
        if first && matches!(self.state, State::Initializing) {
            self.state = State::Idle;
        }
        let _ = sender.send(Ok(response));
        if first {
            self.release_held_commands().await?;
        }
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
            Frame::Unparsed => self.set_error_result(None),
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
                        self.set_error_result(Some(crate::error::error_result_data(result)));
                    }
                    Message::System(system) if system.subtype == "session_state_changed" => {}
                    _ => self.set_error_result(None),
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
                // TypeScript `dropUnreadHookProgress`: while `startup()` waits, hook progress is not kept.
                if self.startup_tap.as_ref().is_some_and(|tap| !tap.is_closed())
                    && let Message::System(system) = &message
                    && system.subtype == "hook_progress"
                {
                    return Ok(());
                }
                self.run_state.observe(&message, queued_limit, task_limit)?;
                let flushing = matches!(message, Message::Result(_)) && self.session.is_some();
                if flushing || self.mirror_flush.is_some() || !self.held_observations.is_empty() {
                    self.held_observations.push_back((message.clone(), flushing));
                } else {
                    let needs = self.bidirectional_needs();
                    self.run_end.observe(&message, needs);
                }
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
                                return self.finish_initialize(first, sender, response).await;
                            }
                            ControlResponder::Raw { response: sender, .. } => {
                                let _ = sender.send(Ok(response));
                                return Ok(());
                            }
                            ControlResponder::Control { response: sender, .. } => {
                                (control_value(self.argv_style, response), sender)
                            }
                        };
                        if operation == ControlOperation::RemoteControl {
                            self.host.settle_remote_control(response.as_ref());
                        }
                        if operation == ControlOperation::Interrupt
                            && let Some(object) = response.as_ref().and_then(serde_json::Value::as_object)
                            && let crate::InterruptReceipt::Reported {
                                cancelled: Some(cancelled),
                                ..
                            } = crate::InterruptReceipt::from_json(crate::deep_json::clone_object(object))
                        {
                            self.run_state
                                .pending_inputs
                                .retain(|input| input.as_ref().is_none_or(|input| !cancelled.contains(input)));
                        }
                        let _ = sender.send(Ok(response));
                    }
                    ControlResponse::Error { message, error_code } => {
                        let first_initialize =
                            matches!(pending.response, ControlResponder::Initialize { first: true, .. });
                        pending.response.send_error(Error::ControlRejected {
                            operation,
                            rejection: ControlRejectionError::new(message.clone(), error_code.clone(), self.argv_style),
                        });
                        // The hold turns a failed first initialize into the end of the run, with that error.
                        if first_initialize && self.initialize_hold {
                            self.abandon_initialize(&std::sync::Arc::new(Error::ControlRejected {
                                operation,
                                rejection: ControlRejectionError::new(
                                    message.clone(),
                                    error_code.clone(),
                                    self.argv_style,
                                ),
                            }));
                            return Err(Error::ControlRejected {
                                operation,
                                rejection: ControlRejectionError::new(message, error_code, self.argv_style),
                            });
                        }
                    }
                }
            }
            Frame::ControlRequest { id, request } => {
                if self.admit_request(&id)? {
                    self.control_request(id, request).await?;
                }
            }
            Frame::Cancel { id } => {
                if let Some(handle) = self.callback_handles.remove(&id) {
                    if let Some(signal) = self.callback_signals.remove(&id) {
                        signal.cancel();
                    }
                    match self.argv_style {
                        // TypeScript aborts the request's signal and forgets the id; the handler keeps running and
                        // its answer is still written.
                        crate::ArgvStyle::TypeScript => {
                            self.cancelled_callbacks.insert(handle.id());
                        }
                        // Python cancels the handler task, which writes nothing.
                        crate::ArgvStyle::Python => handle.abort(),
                    }
                }
            }
            Frame::TranscriptMirror { file_path, entries } => {
                if let Some(session) = &self.session {
                    match session.mirror.enqueue(&file_path, entries) {
                        Ok(()) => {}
                        Err(crate::MirrorEnqueueError::Key(error)) => {
                            // One line for the same reason as the control response above.
                            let projects_dir = session.mirror.projects_dir().unwrap_or(std::path::Path::new(""));
                            tracing::warn!(file_path = %file_path.display(), projects_dir = %projects_dir.display(), %error, "dropping transcript mirror frame whose file path is not under the projects directory");
                        }
                        Err(error @ crate::MirrorEnqueueError::MissingConfigDirectory) => {
                            // Upstream TypeScript's append throws from `os.homedir()`, is logged at error level, and
                            // its `onError` throws again before it can report a `mirror_error`.
                            tracing::error!(file_path = %file_path.display(), %error, "dropping transcript mirror frame");
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
                _ = &mut self.shutdown, if !self.shutdown_seen => { self.shutdown_seen = true; false }
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

    /// `sdk.mjs` `mcp_message`: an unknown server or a message that is not an object fails the control request.
    /// A request registers a waiter under `${server_name}:${id}` and waits for the server's transport to send a
    /// message under that id, however long it takes; a second request under the same key replaces the first
    /// waiter, which is then never answered. Anything else is handed to the server and acknowledged at once.
    async fn typescript_mcp_message(
        &mut self,
        id: ControlRequestId,
        server_name: Option<serde_json::Value>,
        message: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        let (name, session) = match &server_name {
            Some(serde_json::Value::String(name)) => (
                name.clone(),
                self.sdk_mcp_servers
                    .get(&crate::McpServerName::from(name.as_str()))
                    .cloned(),
            ),
            _ => (String::new(), None),
        };
        let Some(session) = session else {
            let text = format!(
                "SDK MCP server not found: {}",
                crate::mcp_routing::js_string(server_name.as_ref())
            );
            return self.write_response(wire::callback_error_response(&id, &text)).await;
        };
        let message = match message {
            Some(serde_json::Value::Object(message)) => Some(message),
            // An array is an object to `in`, so it is handed over like a notification.
            Some(array @ serde_json::Value::Array(_)) => {
                crate::deep_json::drop_value(array);
                None
            }
            other => {
                let text = crate::mcp_routing::typescript_not_an_object(other.as_ref(), self.js_runtime);
                return self.write_response(wire::callback_error_response(&id, &text)).await;
            }
        };
        let expects_response = message
            .as_ref()
            .is_some_and(crate::mcp_routing::typescript_expects_response);
        if expects_response && let Some(message) = message {
            if !self.admit_capacity(&id).await? {
                return Ok(());
            }
            let key = format!("{name}:{}", crate::mcp_routing::js_string(message.get("id")));
            let (waiter, answer) = oneshot::channel();
            // Replacing the entry drops the earlier waiter, whose control request is never answered.
            self.mcp.waiters.insert(key.clone(), waiter);
            let signal = crate::CancellationToken::new();
            let callback_signal = signal.clone();
            let response_id = id.clone();
            let fallback = wire::callback_error_response(&id, "SDK MCP request failed").map(Some);
            let outbound = self.mcp.outbound.clone();
            self.mcp.tasks.spawn(async move {
                match session.request(message, callback_signal).await {
                    Ok(Some(message)) => {
                        let _ = outbound.send(McpTransportEvent::Message { server: name, message });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::debug!(%error, "SDK MCP server failed to handle a request");
                        let _ = outbound.send(McpTransportEvent::Failed { key });
                    }
                }
            });
            // `sdk.mjs` has no timeout on this wait: it ends when the server answers or the query closes.
            self.spawn_callback(id, signal, None, fallback, async move {
                match answer.await {
                    Ok(Ok(response)) => wire::success_response(
                        &response_id,
                        McpResponsePayload {
                            mcp_response: serde_json::Value::Object(response),
                        },
                    )
                    .map(Some),
                    Ok(Err(())) => wire::callback_error_response(&response_id, "SDK MCP request failed").map(Some),
                    Err(_) => pending().await,
                }
            });
            return Ok(());
        }
        if let Some(message) = message {
            // `transport.onmessage` runs the server's handling up to its first wait before the acknowledgment is
            // written; whatever is left keeps running on its own.
            let mut delivery = Box::pin(async move {
                match session.request(message, crate::CancellationToken::new()).await {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::debug!(%error, "SDK MCP server failed to handle a message");
                        None
                    }
                }
            });
            let polled = std::future::poll_fn(|context| std::task::Poll::Ready(delivery.as_mut().poll(context))).await;
            match polled {
                std::task::Poll::Ready(Some(message)) => self.route_mcp_message(name, message).await?,
                std::task::Poll::Ready(None) => {}
                std::task::Poll::Pending => {
                    let outbound = self.mcp.outbound.clone();
                    self.mcp.tasks.spawn(async move {
                        if let Some(message) = delivery.await {
                            let _ = outbound.send(McpTransportEvent::Message { server: name, message });
                        }
                    });
                }
            }
        }
        self.write_response(wire::success_response(
            &id,
            McpResponsePayload {
                mcp_response: crate::mcp_routing::typescript_acknowledgment(),
            },
        ))
        .await
    }

    /// `sdk.mjs` `captureSdkMcpServerManifests`: handshakes every in-process server at once, waiting at most
    /// `$8` for all of them, while what their transports send is routed as usual.
    async fn capture_sdk_mcp_manifests(&mut self) -> Result<Option<JsonObject>, Error> {
        let started = std::time::Instant::now();
        let mut tasks = JoinSet::new();
        for (name, session) in &self.sdk_mcp_servers {
            tasks.spawn(
                mcp_manifest::Capture {
                    name: name.as_str().to_owned(),
                    session: session.clone(),
                    waiters: self.mcp.waiters.clone(),
                    outbound: self.mcp.outbound.clone(),
                }
                .run(),
            );
        }
        let servers = tasks.len();
        let mut captured = HashMap::new();
        let deadline = sleep(constants::MCP_MANIFEST_CAPTURE_DEADLINE);
        tokio::pin!(deadline);
        let mut settled = true;
        loop {
            tokio::select! {
                joined = tasks.join_next() => match joined {
                    None => break,
                    Some(Ok(Some((name, manifest)))) => {
                        captured.insert(name, manifest);
                    }
                    Some(_) => {}
                },
                event = self.mcp.inbound.recv() => {
                    if let Some(event) = event {
                        Box::pin(self.mcp_transport_event(event)).await?;
                    }
                }
                () = &mut deadline => {
                    settled = false;
                    break;
                }
                _ = &mut self.shutdown, if !self.shutdown_seen => {
                    self.shutdown_seen = true;
                    return Err(Error::ShutdownRequested);
                }
            }
        }
        if !settled {
            // `sdk.mjs` routes each transport message and records each finished handshake the moment it happens, so
            // whatever is already queued when the deadline fires still counts.
            while let Ok(event) = self.mcp.inbound.try_recv() {
                self.mcp_transport_event(event).await?;
            }
            while let Some(joined) = tasks.try_join_next() {
                if let Ok(Some((name, manifest))) = joined {
                    captured.insert(name, manifest);
                }
            }
        }
        // `n = true`: replies that come later are ignored, and the drop rule of `route_mcp_message` takes them.
        tasks.abort_all();
        let elapsed = started.elapsed().as_millis();
        if !settled {
            tracing::warn!(
                captured = captured.len(),
                servers,
                deadline_ms = constants::MCP_MANIFEST_CAPTURE_DEADLINE.as_millis(),
                "in-process MCP servers captured within the deadline; the rest will handshake over the control channel"
            );
        }
        if captured.is_empty() {
            return Ok(None);
        }
        let manifests: JsonObject = self
            .sdk_mcp_servers
            .keys()
            .filter_map(|name| {
                captured
                    .remove(name.as_str())
                    .map(|manifest| (name.as_str().to_owned(), serde_json::Value::Object(manifest)))
            })
            .collect();
        tracing::debug!(servers = ?manifests.keys().collect::<Vec<_>>(), elapsed_ms = %elapsed, "SDK MCP manifests captured");
        Ok(Some(manifests))
    }

    /// `sdk.mjs` `sendMcpServerMessageToCli`: a message from a server's transport resolves the waiter under
    /// `${server_name}:${id}`; with no waiter, a late `sdk-manifest-capture:` reply is dropped and anything else
    /// (a notification, a server request, an unmatched or repeated response) goes to the CLI as an `mcp_message`
    /// control request under a random UUID.
    /// What an in-process server's transport sent: a message to route, or a failed send that fails its waiter.
    async fn mcp_transport_event(&mut self, event: McpTransportEvent) -> Result<(), Error> {
        match event {
            McpTransportEvent::Message { server, message } => self.route_mcp_message(server, message).await,
            McpTransportEvent::Failed { key } => {
                if let Some(waiter) = self.mcp.waiters.remove(&key) {
                    let _ = waiter.send(Err(()));
                }
                Ok(())
            }
        }
    }

    async fn route_mcp_message(&mut self, server: String, message: JsonObject) -> Result<(), Error> {
        if let Some(id) = message.get("id").filter(|id| !id.is_null()) {
            let key = format!("{server}:{}", crate::mcp_routing::js_string(Some(id)));
            if let Some(waiter) = self.mcp.waiters.remove(&key) {
                let _ = waiter.send(Ok(message));
                return Ok(());
            }
            if matches!(id, serde_json::Value::String(text) if text.starts_with("sdk-manifest-capture:")) {
                tracing::debug!(%server, "dropping a late manifest-capture reply");
                return Ok(());
            }
        }
        let id = ControlRequestId::random_uuid();
        let request = RequestPayload::McpMessage {
            server_name: &server,
            message: &message,
        }
        .to_value()?;
        let frame = wire::mcp_forward(&id, request)?;
        if !self.mcp.initialize_written {
            if crate::mcp_routing::is_tools_list_changed(&message) {
                self.mcp.tools_changed.insert(server);
            }
            self.mcp.held.push_back(frame);
            return Ok(());
        }
        self.write_response(Ok(frame)).await
    }

    /// The Python `Query` `mcp_message`: a missing field fails the control request, and everything else is
    /// answered with an `mcp_response`, a JSON-RPC error when the message could not be delivered.
    async fn python_mcp_message(
        &mut self,
        id: ControlRequestId,
        server_name: Option<serde_json::Value>,
        message: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        let ids = self.wire_ids.take().unwrap_or_default();
        let truthy = crate::python_json::is_truthy;
        let (Some(server_name), Some(message)) = (server_name.filter(truthy), message.filter(truthy)) else {
            return self
                .write_response(wire::callback_error_response(
                    &id,
                    "Missing server_name or message for MCP request",
                ))
                .await;
        };
        // The `assert isinstance` checks fail with an `AssertionError` whose text is empty.
        let (serde_json::Value::String(server_name), serde_json::Value::Object(mut message)) = (server_name, message)
        else {
            return self.write_response(wire::callback_error_response(&id, "")).await;
        };
        ids.apply_to(&mut message);
        let Some(session) = self
            .sdk_mcp_servers
            .get(&crate::McpServerName::from(server_name.as_str()))
            .cloned()
        else {
            let mut mcp_response =
                crate::mcp_routing::python_error(&message, -32601, format!("Server '{server_name}' not found"));
            ids.echo(&mut mcp_response);
            crate::deep_json::clear_object(&mut message);
            return self
                .write_response(wire::success_response(&id, McpResponsePayload { mcp_response }))
                .await;
        };
        if !self.admit_capacity(&id).await? {
            return Ok(());
        }
        let signal = crate::CancellationToken::new();
        let callback_signal = signal.clone();
        let response_id = id.clone();
        let fallback = wire::callback_error_response(&id, "SDK MCP request timed out").map(Some);
        self.spawn_callback(id, signal, self.callback_timeout(), fallback, async move {
            let mut mcp_response = match session
                .request_python(crate::deep_json::clone_object(&message), callback_signal)
                .await
            {
                Ok(Some(response)) => crate::PythonJsonValue::Object(response),
                Ok(None) => crate::mcp_routing::python_acknowledgment().into(),
                Err(crate::McpServerError::Protocol(text)) => {
                    crate::mcp_routing::python_error(&message, -32603, text).into()
                }
                Err(error) => crate::mcp_routing::python_error(&message, -32603, error.to_string()).into(),
            };
            ids.echo_python(&mut mcp_response);
            crate::deep_json::clear_object(&mut message);
            // The response goes in once the frame is converted, since `serde_json` cannot carry it.
            let frame = wire::success_response(
                &response_id,
                McpResponsePayload {
                    mcp_response: serde_json::Value::Null,
                },
            );
            frame.map(|frame| python_mcp_frame(frame, mcp_response)).map(Some)
        });
        Ok(())
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

    /// A flush finished: the run-end machine sees the result that started it and the frames after it, up to the
    /// next result that starts a flush of its own.
    fn release_observations(&mut self) {
        let mut released_flush = false;
        while let Some((_, flushing)) = self.held_observations.front() {
            if *flushing {
                if released_flush {
                    break;
                }
                released_flush = true;
            }
            let Some((message, _)) = self.held_observations.pop_front() else {
                break;
            };
            let needs = self.bidirectional_needs();
            self.run_end.observe(&message, needs);
        }
    }

    async fn finish_mirror(&mut self) -> Result<(), Error> {
        while let Some(flush) = self.mirror_flush.take() {
            let failures = flush.await?;
            self.release_observations();
            for failure in failures {
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

    fn spawn_callback<Fut, Out>(
        &mut self,
        id: ControlRequestId,
        signal: crate::CancellationToken,
        deadline: Option<Duration>,
        fallback: Result<Option<serde_json::Value>, ProtocolError>,
        future: Fut,
    ) where
        Fut: Future<Output = Result<Option<Out>, ProtocolError>> + Send + 'static,
        Out: Into<Outgoing> + Send + 'static,
    {
        let callback_id = id.clone();
        let callback_signal = signal.clone();
        let handle = self.callbacks.spawn(async move {
            let response = match deadline {
                None => future.await.map(|frame| frame.map(Into::into)),
                Some(deadline) => match timeout(deadline, future).await {
                    Ok(response) => response.map(|frame| frame.map(Into::into)),
                    Err(_) => {
                        callback_signal.cancel();
                        fallback.map(|frame| frame.map(Outgoing::from))
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
        response: Result<Option<Outgoing>, ProtocolError>,
    ) -> Result<(), Error> {
        match response.transpose() {
            Some(frame) => self.write_response(frame).await,
            None => Ok(()),
        }
    }

    async fn write_response(&mut self, frame: Result<impl Into<Outgoing>, ProtocolError>) -> Result<(), Error> {
        let frame = frame?;
        if !self.input_open() {
            // The run has settled and input is closed: a late callback answer has nowhere
            // to go, and that must not stop the driver.
            tracing::debug!("dropping a control response written after stdin closed");
            return Ok(());
        }
        self.write(frame).await
    }

    async fn write(&mut self, frame: impl Into<Outgoing>) -> Result<(), Error> {
        // Each SDK writes its own JSON spelling: TypeScript `JSON.stringify`, Python `json.dumps`.
        let mut text = match frame.into() {
            Outgoing::Json(frame) => match self.argv_style {
                crate::ArgvStyle::TypeScript => crate::js_json::to_string(&frame),
                crate::ArgvStyle::Python => {
                    crate::python_json::to_string(&frame).map(crate::mcp_wire_ids::splice_markers)
                }
            }
            .map_err(ProtocolError::from)?,
            // Only the Python argv style builds these.
            Outgoing::Python(frame) => crate::mcp_wire_ids::splice_markers(crate::python_json_dumps(&frame)),
        };
        text.push('\n');
        if !self.input_open() {
            return Err(Error::NotReadyForWriting);
        }
        let limit = self.limits.write_timeout.map(crate::Timeout::as_duration);
        // The driver took this frame before it saw any close, so the caller handed it over first. `sdk.mjs` writes
        // such a frame to stdin at once and `close()` only ends the stream behind it, so the write is polled first
        // and goes out when the transport takes it right away. Only a write that has to wait gives way to the close
        // or the abort, as a JavaScript write still awaiting its transport does.
        tokio::select! {
            biased;
            result = async { match limit { Some(limit) => timeout(limit, self.transport.write(&text)).await.map_err(|_| Error::WriteTimeout)?, None => self.transport.write(&text).await } } => result,
            () = self.abort.cancelled() => Err(crate::AbortError::PROCESS.into()),
            _ = &mut self.shutdown, if !self.shutdown_seen => { self.shutdown_seen = true; Err(Error::ShutdownRequested) }
        }
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
