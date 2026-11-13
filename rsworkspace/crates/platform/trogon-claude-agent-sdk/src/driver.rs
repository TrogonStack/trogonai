use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    future::{Future, pending},
    sync::Arc,
    time::Duration,
};

use tokio::{
    sync::{mpsc, oneshot},
    task::{AbortHandle, JoinHandle, JoinSet},
    time::{Instant, sleep_until, timeout},
};

use crate::{
    ClaudeAgentOptions, ControlOperation, ControlRejectionError, ControlRequestId, Error, JsonObject, Limits, Message,
    PermissionDecision, ProtocolError, StderrTail, SystemPrompt, ToolPermissionHandler, Transport,
    wire::{
        self, ControlResponse, Frame, IncomingControl,
        runtime_payload::{
            EmbeddedMcpPayload, HookMatcherPayload, HooksPayload, InitializeRequest, McpResponsePayload, RequestPayload,
        },
    },
};

pub(crate) enum Command {
    Initialize {
        response: oneshot::Sender<Result<crate::ConnectionInfo, Error>>,
    },
    SetMcpServers {
        external: BTreeMap<crate::McpServerName, crate::McpServerOptions>,
        embedded: BTreeMap<crate::McpServerName, Arc<dyn crate::SdkMcpServer>>,
        response: oneshot::Sender<Result<JsonObject, Error>>,
    },
    Prompt {
        input: crate::UserInput,
        response: oneshot::Sender<Result<(), Error>>,
    },
    Control {
        operation: ControlOperation,
        request: serde_json::Value,
        response: oneshot::Sender<Result<JsonObject, Error>>,
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
    fn observe(&mut self, message: &Message, queued_limit: usize, task_limit: usize) -> Result<(), Error> {
        match message {
            Message::Result(message) => {
                self.result_received = true;
                self.turn_active = false;
                self.awaiting_continuation = false;
                if let Some(count) = message.queued_turn_count {
                    let count = usize::try_from(count).map_err(|_| Error::TooManyInputs)?;
                    if count > queued_limit {
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
                        if self.tasks.len() >= task_limit && !self.tasks.contains(id) {
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
    deadline: Instant,
}

enum ControlResponder {
    Initialize(oneshot::Sender<Result<crate::ConnectionInfo, Error>>),
    Control {
        operation: ControlOperation,
        response: oneshot::Sender<Result<JsonObject, Error>>,
    },
}

impl ControlResponder {
    fn operation(&self) -> ControlOperation {
        match self {
            Self::Initialize(_) => ControlOperation::Initialize,
            Self::Control { operation, .. } => *operation,
        }
    }

    fn send_error(self, error: Error) {
        match self {
            Self::Initialize(response) => {
                let _ = response.send(Err(error));
            }
            Self::Control { response, .. } => {
                let _ = response.send(Err(error));
            }
        }
    }
}

type CallbackOutput = (ControlRequestId, Result<serde_json::Value, ProtocolError>);

type McpSessions = BTreeMap<crate::McpServerName, Arc<dyn crate::SdkMcpSession>>;

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
    commands: mpsc::Receiver<Command>,
    messages: mpsc::Sender<Message>,
    shutdown: oneshot::Receiver<()>,
    limits: Limits,
    handler: Option<Arc<dyn ToolPermissionHandler>>,
    pending: HashMap<ControlRequestId, PendingControl>,
    callbacks: JoinSet<CallbackOutput>,
    callback_handles: HashMap<ControlRequestId, AbortHandle>,
    callback_signals: HashMap<ControlRequestId, crate::CancellationToken>,
    hook_callbacks: HashMap<String, HookCallback>,
    sdk_mcp_servers: McpSessions,
    pending_mcp: Option<(ControlRequestId, McpSessions)>,
    initialize_request: serde_json::Value,
    initialize_expectations: InitializeExpectations,
    elicitation_handler: Option<Arc<dyn crate::ElicitationHandler>>,
    user_dialog_handler: Option<Arc<dyn crate::UserDialogHandler>>,
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
}

type MirrorBarrier =
    std::pin::Pin<Box<dyn Future<Output = Result<Vec<crate::MirrorFailureError>, crate::SessionError>> + Send>>;

#[derive(Clone)]
struct HookCallback {
    handler: Arc<dyn crate::HookHandler>,
    timeout: Duration,
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

/// The object a control response carries, with an absent or null `response` read as an empty one.
fn control_object(response: serde_json::Value) -> Result<JsonObject, Error> {
    match response {
        serde_json::Value::Null => Ok(JsonObject::new()),
        other => serde_json::from_value(other).map_err(|error| ProtocolError::from(error).into()),
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
    if options
        .mcp_servers
        .keys()
        .any(|name| options.sdk_mcp_servers.contains_key(name))
    {
        return Err(Error::DuplicateMcpServer);
    }
    if !options.supported_dialog_kinds.is_empty() && options.user_dialog_handler.is_none() {
        return Err(Error::DialogKindsWithoutHandler);
    }
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
                    timeout: registration
                        .timeout
                        .unwrap_or(options.limits.control_timeout)
                        .as_duration(),
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
    // TypeScript delivers the system prompt in the initialize request, Python on the command line.
    let (system_prompt, append_system_prompt) = match &options.system_prompt {
        SystemPrompt::Empty => (Some(vec![""]), None),
        SystemPrompt::Custom(prompt) => (Some(vec![prompt.as_str()]), None),
        SystemPrompt::ClaudeCode => (None, None),
        SystemPrompt::ClaudeCodeWithAppend(append) => (None, Some(append.as_str())),
    };
    let request = InitializeRequest {
        hooks: (!options.hooks.is_empty()).then_some(hooks),
        json_schema: options.output_schema.as_ref().map(crate::OutputSchema::as_json),
        system_prompt,
        append_system_prompt,
        agents,
        supported_dialog_kinds,
        plugins: plugins_via_initialize(options)
            .then(|| options.plugins.iter().map(crate::plugin::PluginPayload::from).collect()),
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
    pub commands: mpsc::Receiver<Command>,
    pub messages: mpsc::Sender<Message>,
    pub shutdown: oneshot::Receiver<()>,
    pub terminal: oneshot::Sender<Result<(), Error>>,
    pub transport: T,
    pub initialization: Initialization,
    /// The CLI path to check against the minimum version, for a transport that runs the CLI itself.
    pub version_check: Option<String>,
}

/// The initialize request and the callbacks it registers, built before any transport exists so that a bad
/// configuration fails before a process is spawned.
pub(crate) struct Initialization {
    request: serde_json::Value,
    hook_callbacks: HashMap<String, HookCallback>,
    expectations: InitializeExpectations,
}

impl Initialization {
    pub(crate) fn new(options: &ClaudeAgentOptions) -> Result<Self, Error> {
        let (request, hook_callbacks, expectations) = initialize_config(options)?;
        Ok(Self {
            request,
            hook_callbacks,
            expectations,
        })
    }
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
    } = input;
    transport.connect().await?;
    let driver = Driver {
        transport,
        input_ended: false,
        commands,
        messages,
        shutdown,
        limits: options.limits,
        handler: options.permission_handler,
        pending: HashMap::new(),
        callbacks: JoinSet::new(),
        callback_handles: HashMap::new(),
        callback_signals: HashMap::new(),
        hook_callbacks: initialization.hook_callbacks,
        sdk_mcp_servers: options
            .sdk_mcp_servers
            .iter()
            .map(|(name, server)| (name.clone(), server.open_session()))
            .collect(),
        pending_mcp: None,
        initialize_request: initialization.request,
        initialize_expectations: initialization.expectations,
        elicitation_handler: options.elicitation_handler,
        user_dialog_handler: options.user_dialog_handler,
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
        for signal in self.callback_signals.values() {
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
                return Ok(());
            }
            if let Some(operation) = self.expired_control() {
                return Err(Error::ControlTimeout { operation });
            }
            if self.run_state.settled(self.callback_handles.len()) && self.mirror_flush.is_none() {
                self.end_input().await;
            }
            let between_turns = self.run_state.input_done.is_some()
                && self.run_state.result_received
                && !self.run_state.turn_active
                && self.callback_handles.is_empty()
                && self.run_state.tasks.is_empty()
                && !matches!(self.run_state.session_state, Some(SessionState::RequiresAction));
            if between_turns && self.input_open() {
                self.run_state
                    .deadline
                    .get_or_insert_with(|| Instant::now() + self.limits.run_end_timeout.as_duration());
            } else {
                self.run_state.deadline = None;
            }
            let deadline = self.pending.values().map(|request| request.deadline).min();
            let run_deadline = self.run_state.deadline;
            tokio::select! {
                _ = &mut self.shutdown => return Ok(()),
                message = self.transport.read_message() => {
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
                            self.write_response(response).await?;
                        }
                        Some(Err(error)) if error.is_cancelled() => {}
                        Some(Err(error)) if self.callback_handles.values().any(|handle| handle.id() == error.id()) => return Err(Error::CallbackTask(error)),
                        _ => {}
                    }
                }
                _ = async { match deadline { Some(deadline) => sleep_until(deadline).await, None => pending::<()>().await } } => {
                    if let Some(operation) = self.expired_control() { return Err(Error::ControlTimeout { operation }); }
                }
                _ = async { match run_deadline { Some(deadline) => sleep_until(deadline).await, None => pending::<()>().await } } => {
                    tracing::debug!(timeout = ?self.limits.run_end_timeout.as_duration(), "no idle state after the last result; ending the run");
                    self.run_state.deadline = None;
                    self.end_input().await;
                }
            }
        }
    }

    fn expired_control(&mut self) -> Option<ControlOperation> {
        let now = Instant::now();
        let id = self
            .pending
            .iter()
            .find_map(|(id, request)| (request.deadline <= now).then(|| id.clone()))?;
        let request = self.pending.remove(&id)?;
        let operation = request.response.operation();
        request.response.send_error(Error::ControlTimeout { operation });
        Some(operation)
    }

    async fn admit_callback(&mut self, id: &ControlRequestId) -> Result<bool, Error> {
        if self.callback_handles.contains_key(id) {
            return Err(ProtocolError::DuplicateControlRequest.into());
        }
        if self.callback_handles.len() >= self.limits.max_pending_controls.get() {
            self.write_response(wire::callback_error_response(id, "Too many outstanding SDK callbacks"))
                .await?;
            return Ok(false);
        }
        Ok(true)
    }

    async fn command(&mut self, command: Command) -> Result<(), Error> {
        match command {
            Command::Initialize { response } => {
                self.start_control(ControlResponder::Initialize(response), self.initialize_request.clone())
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
                if self.pending_mcp.is_some() {
                    let _ = response.send(Err(Error::McpUpdateInProgress));
                    return Ok(());
                }
                if self.pending.len() >= self.limits.max_pending_controls.get() {
                    let _ = response.send(Err(Error::TooManyControls));
                    return Ok(());
                }
                let mut servers = serde_json::Map::new();
                for (name, config) in external {
                    servers.insert(
                        name.as_str().to_owned(),
                        config.to_wire_json().map_err(ProtocolError::from)?,
                    );
                }
                let mut sessions = BTreeMap::new();
                for (name, registry) in embedded {
                    if servers.contains_key(name.as_str()) {
                        let _ = response.send(Err(Error::DuplicateMcpServer));
                        return Ok(());
                    }
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
                let id = self.next_request_id()?;
                let frame = wire::control_request(
                    &id,
                    RequestPayload::McpSetServers { servers }.to_value()?,
                    self.argv_style,
                )?;
                self.pending_mcp = Some((id.clone(), sessions));
                self.write(frame).await?;
                self.pending.insert(
                    id,
                    PendingControl {
                        response: ControlResponder::Control {
                            operation: ControlOperation::McpSetServers,
                            response,
                        },
                        deadline: Instant::now() + self.limits.control_timeout.as_duration(),
                    },
                );
            }
            Command::Prompt { input, response } => {
                if !self.input_open() {
                    let _ = response.send(Err(Error::NotReadyForWriting));
                    return Ok(());
                }
                if self.run_state.input_done.is_some() {
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
                if self.run_state.pending_inputs.len() >= self.limits.command_queue_capacity.get() {
                    let _ = response.send(Err(Error::TooManyInputs));
                    return Ok(());
                }
                let frame = input.to_wire_json().map_err(ProtocolError::from)?;
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
            Command::FinishInput { completion, response } => {
                self.run_state.input_done = Some(completion);
                let _ = response.send(Ok(()));
            }
            Command::Abort => return Err(crate::AbortError::PROCESS.into()),
        }
        Ok(())
    }

    /// Applies the initialize checks of `sdk.mjs` `Query.initialize`, in its order.
    fn finish_initialize(
        &mut self,
        sender: oneshot::Sender<Result<crate::ConnectionInfo, Error>>,
        response: serde_json::Value,
    ) -> Result<(), Error> {
        // Python reads anything but an object as `{}`; TypeScript keeps the value as sent.
        let response = match (self.argv_style, response) {
            (crate::ArgvStyle::Python, serde_json::Value::Object(object)) => serde_json::Value::Object(object),
            (crate::ArgvStyle::Python, _) => serde_json::Value::Object(JsonObject::new()),
            (crate::ArgvStyle::TypeScript, response) => response,
        };
        let connection_info = crate::ConnectionInfo::from_json(response);
        let expectations = self.initialize_expectations;
        let refusal: Option<fn() -> Error> = if expectations.blocking_hooks
            && connection_info.hook_on_failure_supported != crate::FeatureSupport::Supported
        {
            Some(|| Error::BlockingHooksUnsupported)
        } else if expectations.blocking_hooks && connection_info.hooks_applied == crate::FeatureSupport::Unsupported {
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
                "[Query.initialize] Claude Code reported plugins_applied={} for {} plugins sent with pluginDelivery 'initialize'; the process is running with the plugins it was launched with.",
                javascript_string(connection_info.plugins_applied),
                expectations.plugins
            );
        }
        self.state = State::Idle;
        let _ = sender.send(Ok(connection_info));
        Ok(())
    }

    /// TypeScript sends a random id per request, Python `req_{counter}_{random}`.
    fn next_request_id(&mut self) -> Result<ControlRequestId, Error> {
        self.next_id = self.next_id.checked_add(1).ok_or(Error::RequestIdExhausted)?;
        Ok(match self.argv_style {
            crate::ArgvStyle::TypeScript => ControlRequestId::typescript(),
            crate::ArgvStyle::Python => ControlRequestId::python(self.next_id),
        })
    }

    async fn start_control(&mut self, response: ControlResponder, request: serde_json::Value) -> Result<(), Error> {
        if !self.input_open() {
            response.send_error(Error::NotReadyForWriting);
            return Ok(());
        }
        if self.pending.len() >= self.limits.max_pending_controls.get() {
            response.send_error(Error::TooManyControls);
            return Ok(());
        }
        let id = self.next_request_id()?;
        self.write(wire::control_request(&id, request, self.argv_style)?)
            .await?;
        self.pending.insert(
            id,
            PendingControl {
                response,
                deadline: Instant::now() + self.limits.control_timeout.as_duration(),
            },
        );
        Ok(())
    }

    async fn frame(&mut self, frame: Frame) -> Result<(), Error> {
        match frame {
            Frame::KeepAlive => {}
            // Python clears the pending error result for every message it does not parse.
            Frame::Unparsed => self.last_error_result = None,
            Frame::Message(message) => {
                let (queued_limit, task_limit) = (
                    self.limits.command_queue_capacity.get(),
                    self.limits.message_queue_capacity.get(),
                );
                if let Message::System(system) = &message
                    && system.subtype == "init"
                    && let Some(cli_path) = self.version_check.take()
                    && let Some(warning) = crate::version_check::unreported_version_warning(
                        system
                            .data
                            .get("claude_code_version")
                            .and_then(serde_json::Value::as_str),
                        &cli_path,
                    )
                {
                    tracing::warn!(%warning, "Claude Code version warning");
                }
                match &message {
                    Message::Result(result) if result.status == crate::ResultStatus::Error => {
                        self.last_error_result = Some(crate::error::error_result_data(result));
                    }
                    Message::System(system) if system.subtype == "session_state_changed" => {}
                    _ => self.last_error_result = None,
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
                    if self.deferred_messages.len() >= self.limits.message_queue_capacity.get() {
                        return Err(Error::MessageQueueFull);
                    }
                    self.deferred_messages.push_back((message, barrier));
                } else {
                    self.publish(message)?;
                }
            }
            Frame::ControlResponse { id, response } => {
                let Some(mut pending) = self.pending.remove(&id) else {
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
                        let response = match pending.response {
                            ControlResponder::Initialize(sender) => return self.finish_initialize(sender, response),
                            responder => {
                                pending.response = responder;
                                control_object(response)?
                            }
                        };
                        if operation == ControlOperation::Interrupt
                            && let crate::InterruptReceipt::Reported {
                                cancelled: Some(cancelled),
                                ..
                            } = crate::InterruptReceipt::from_json(response.clone())
                        {
                            self.run_state
                                .pending_inputs
                                .retain(|input| input.as_ref().is_none_or(|input| !cancelled.contains(input)));
                        }
                        if let Some((_, mut sessions)) = self.pending_mcp.take_if(|(request_id, _)| *request_id == id) {
                            let result: crate::McpSetServersResult =
                                serde_json::from_value(response.clone().into()).map_err(ProtocolError::from)?;
                            for (name, previous) in &self.sdk_mcp_servers {
                                if result.errors.contains_key(name.as_str()) {
                                    sessions.insert(name.clone(), previous.clone());
                                }
                            }
                            sessions.retain(|name, _| {
                                !result.errors.contains_key(name.as_str()) || self.sdk_mcp_servers.contains_key(name)
                            });
                            self.sdk_mcp_servers = sessions;
                        }
                        match pending.response {
                            ControlResponder::Initialize(_) => {}
                            ControlResponder::Control { response: sender, .. } => {
                                let _ = sender.send(Ok(response));
                            }
                        }
                    }
                    ControlResponse::Error(error) => {
                        if operation == ControlOperation::McpSetServers {
                            self.pending_mcp = None;
                        }
                        pending.response.send_error(Error::ControlRejected {
                            operation,
                            rejection: ControlRejectionError(error),
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
                    let input = request.input.clone();
                    let signal = request.signal.clone();
                    if let Some(handler) = self
                        .handler
                        .clone()
                        .filter(|_| self.callback_handles.len() < self.limits.max_pending_controls.get())
                    {
                        let callback_id = id.clone();
                        let fallback = wire::permission_response(
                            &id,
                            input.clone(),
                            PermissionDecision::deny("Permission callback timed out"),
                        );
                        self.spawn_callback(id, signal, self.limits.control_timeout.as_duration(), fallback, async move {
                                wire::permission_response(&callback_id, input, handler.decide(request).await)
                            });
                    } else {
                        self.write_response(wire::permission_response(
                            &id,
                            input,
                            PermissionDecision::deny("No permission handler is available"),
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
                        let fallback = wire::callback_error_response(&id, "SDK hook callback timed out");
                        self.spawn_callback(id, signal, callback.timeout, fallback, async move {
                            match callback.handler.call(request).await {
                                Ok(output) => wire::success_response(&response_id, output.to_wire_json()?),
                                Err(_) => wire::callback_error_response(&response_id, "SDK hook callback failed"),
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
                    let server = self
                        .pending_mcp
                        .as_ref()
                        .and_then(|(_, sessions)| sessions.get(&name))
                        .or_else(|| self.sdk_mcp_servers.get(&name))
                        .cloned();
                    if let Some(server) = server {
                        let signal = crate::CancellationToken::new();
                        let callback_signal = signal.clone();
                        let response_id = id.clone();
                        let fallback = wire::callback_error_response(&id, "SDK MCP request timed out");
                        self.spawn_callback(
                            id,
                            signal,
                            self.limits.control_timeout.as_duration(),
                            fallback,
                            async move {
                                match server.request(message, callback_signal).await {
                                    Ok(response) => {
                                        wire::success_response(&response_id, McpResponsePayload::new(response)?)
                                    }
                                    Err(_) => wire::callback_error_response(&response_id, "SDK MCP request failed"),
                                }
                            },
                        );
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
                        );
                        self.spawn_callback(
                            id,
                            signal,
                            self.limits.control_timeout.as_duration(),
                            fallback,
                            async move {
                                let response = handler
                                    .call(request)
                                    .await
                                    .unwrap_or(crate::ElicitationResponse::Cancel);
                                wire::success_response(&response_id, response.to_wire_json()?)
                            },
                        );
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
                    );
                    self.spawn_callback(
                        id,
                        signal,
                        self.limits.control_timeout.as_duration(),
                        fallback,
                        async move {
                            let response = handler
                                .call(request)
                                .await
                                .unwrap_or(crate::UserDialogResponse::Cancelled);
                            wire::success_response(&response_id, response.to_wire_json()?)
                        },
                    );
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

    fn publish(&self, message: Message) -> Result<(), Error> {
        self.messages.try_send(message).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => Error::MessageQueueFull,
            mpsc::error::TrySendError::Closed(_) => Error::DriverStopped,
        })
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
        deadline: Duration,
        fallback: Result<serde_json::Value, ProtocolError>,
        future: Fut,
    ) where
        Fut: Future<Output = Result<serde_json::Value, ProtocolError>> + Send + 'static,
    {
        let callback_id = id.clone();
        let callback_signal = signal.clone();
        let handle = self.callbacks.spawn(async move {
            let response = match timeout(deadline, future).await {
                Ok(response) => response,
                Err(_) => {
                    callback_signal.cancel();
                    fallback
                }
            };
            (callback_id, response)
        });
        self.callback_handles.insert(id.clone(), handle);
        self.callback_signals.insert(id, signal);
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
            crate::ArgvStyle::TypeScript => serde_json::to_string(&frame),
            crate::ArgvStyle::Python => crate::python_json::to_string(&frame),
        }
        .map_err(ProtocolError::from)?;
        text.push('\n');
        if !self.input_open() {
            return Err(Error::NotReadyForWriting);
        }
        tokio::select! {
            _ = &mut self.shutdown => Err(Error::ShutdownRequested),
            result = timeout(self.limits.control_timeout.as_duration(), self.transport.write(&text)) => result.map_err(|_| Error::WriteTimeout)?,
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
