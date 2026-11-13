use crate::constants::SCRIPT_EXTENSIONS;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    future::{Future, pending},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    process::Command as ProcessCommand,
    sync::{mpsc, oneshot},
    task::{AbortHandle, JoinHandle, JoinSet},
    time::{Instant, sleep_until, timeout},
};

use crate::{
    ClaudeAgentOptions, ControlOperation, ControlRejectionError, ControlRequestId, Error, JsonObject, Limits, Message,
    PermissionDecision, ProtocolError, StderrTail, SystemPrompt, ToolPermissionHandler,
    adapters::SpawnProcess,
    argv,
    environment_plan::{self, Inherited},
    framing::{FrameReader, drain_stderr_with},
    wire::{
        self, ControlResponse, Frame, IncomingControl,
        runtime_payload::{EmbeddedMcpPayload, HookMatcherPayload, McpResponsePayload, RequestPayload},
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

#[cfg(all(test, unix))]
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

struct Driver<I, O, F = crate::adapters::NativeFileSystem>
where
    I: AsyncWrite + Unpin + Send + 'static,
    O: AsyncRead + Unpin + Send + 'static,
    F: crate::adapters::SessionFileSystem,
{
    child: tokio::process::Child,
    stdin: Option<I>,
    frames: FrameReader<O>,
    stderr: JoinHandle<Result<StderrTail, std::io::Error>>,
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
    blocking_hooks: bool,
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
    /// The last `is_error` result, kept while nothing else has arrived since, to turn the exit error into a
    /// result error the way both upstream SDKs do.
    last_error_result: Option<JsonObject>,
    closed: Closed,
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

fn initialize_config(
    options: &ClaudeAgentOptions,
) -> Result<(serde_json::Value, HashMap<String, HookCallback>, bool), Error> {
    if options
        .mcp_servers
        .keys()
        .any(|name| options.sdk_mcp_servers.contains_key(name))
    {
        return Err(Error::DuplicateMcpServer);
    }
    let mut hooks: BTreeMap<String, Vec<HookMatcherPayload<'_>>> = BTreeMap::new();
    let mut callbacks = HashMap::new();
    let mut blocking = false;
    for registration in &options.hooks {
        if registration.callbacks.is_empty() {
            continue;
        }
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
        hooks
            .entry(registration.event.as_str().to_owned())
            .or_default()
            .push(HookMatcherPayload {
                matcher: registration.matcher.as_deref(),
                callback_ids: ids,
                on_failure: registration.on_failure.map(crate::HookFailureMode::as_str),
                timeout: registration.timeout.map(|timeout| timeout.as_duration().as_secs_f64()),
            });
        blocking |= registration.on_failure == Some(crate::HookFailureMode::Block);
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
    let supported_dialog_kinds = if options.user_dialog_handler.is_some() && !options.supported_dialog_kinds.is_empty()
    {
        Some(
            options
                .supported_dialog_kinds
                .iter()
                .map(crate::DialogKind::as_str)
                .collect(),
        )
    } else {
        None
    };
    // TypeScript delivers the system prompt in the initialize request, Python on the command line.
    let (system_prompt, append_system_prompt) = match (options.argv_style, &options.system_prompt) {
        (crate::ArgvStyle::Python, _) => (None, None),
        (_, SystemPrompt::Empty) => (Some(vec![""]), None),
        (_, SystemPrompt::Custom(prompt)) => (Some(vec![prompt.as_str()]), None),
        (_, SystemPrompt::ClaudeCode) => (None, None),
        (_, SystemPrompt::ClaudeCodeWithAppend(append)) => (None, Some(append.as_str())),
    };
    let request = RequestPayload::Initialize {
        hooks: (!hooks.is_empty()).then_some(hooks),
        agents,
        supported_dialog_kinds,
        system_prompt,
        append_system_prompt,
    }
    .to_value()?;
    Ok((request, callbacks, blocking))
}

fn is_script(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| SCRIPT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Builds the child command. A script path runs under `script_runtime.executable` (default `node`), as the
/// TypeScript SDK does; any other path is the program itself.
fn command(
    options: &ClaudeAgentOptions,
    cli_path: &std::path::Path,
    inherited: &Inherited,
) -> Result<ProcessCommand, Error> {
    let arguments = argv::build(options)?;
    let mut command = if is_script(cli_path) {
        let runtime = options
            .script_runtime
            .executable
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("node"));
        let mut command = ProcessCommand::new(runtime);
        command.args(&options.script_runtime.arguments);
        command.arg(cli_path);
        command
    } else {
        ProcessCommand::new(cli_path)
    };
    command.args(arguments);
    if let Some(cwd) = &options.cwd {
        command.current_dir(cwd);
    }
    let plan = environment_plan::plan(options, inherited);
    if plan.clear {
        command.env_clear();
    }
    for name in &plan.remove {
        command.env_remove(name);
    }
    command.envs(&plan.set);
    if options.file_checkpointing == crate::FileCheckpointing::Enabled && options.session_store.is_some() {
        return Err(Error::FileCheckpointingWithSessionStore);
    }
    #[cfg(unix)]
    if let Some(name) = &options.user {
        command.uid(crate::run_as::lookup(name).map_err(Error::Io)?);
    } else if let Some(uid) = options.run_as_uid {
        command.uid(uid);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(command)
}

pub(crate) struct SpawnOptions<'a, P, F: crate::adapters::SessionFileSystem> {
    pub options: ClaudeAgentOptions,
    pub session: Option<crate::SessionRuntime<F>>,
    pub commands: mpsc::Receiver<Command>,
    pub messages: mpsc::Sender<Message>,
    pub shutdown: oneshot::Receiver<()>,
    pub terminal: oneshot::Sender<Result<(), Error>>,
    pub process: &'a P,
    pub cli_path: std::path::PathBuf,
    pub inherited: Inherited,
    pub closed: Closed,
}

/// Why the process can no longer be written to, shared with every `ClientHandle` once the driver has stopped.
#[derive(Clone, Default)]
pub(crate) struct Closed(std::sync::Arc<std::sync::Mutex<Option<ClosedReason>>>);

#[derive(Clone)]
enum ClosedReason {
    /// Python: `returncode` is set while `_ready` is still true.
    Terminated { exit_code: Option<i32> },
    /// TypeScript: the `exit` event sets `ready = false` before any later write.
    NotReady,
}

impl Closed {
    fn record(&self, reason: ClosedReason) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(reason);
        }
    }

    /// The write error for a stopped process, or `None` when the run did not record how it ended.
    pub(crate) fn write_error(&self) -> Option<Error> {
        let reason = self.0.lock().ok()?.clone()?;
        Some(match reason {
            ClosedReason::Terminated { exit_code } => Error::WriteToTerminatedProcess { exit_code },
            ClosedReason::NotReady => Error::NotReadyForWriting,
        })
    }
}

pub(crate) fn spawn<P: SpawnProcess<Error = std::io::Error>, F: crate::adapters::SessionFileSystem>(
    input: SpawnOptions<'_, P, F>,
) -> Result<(Option<u32>, JoinHandle<()>), Error> {
    let SpawnOptions {
        options,
        session,
        commands,
        messages,
        shutdown,
        terminal,
        process: process_provider,
        cli_path,
        inherited,
        closed,
    } = input;
    let (initialize_request, hook_callbacks, blocking_hooks) = initialize_config(&options)?;
    let mut process = command(&options, &cli_path, &inherited)?;
    if let Some(config_dir) = session.as_ref().and_then(|session| session.config_dir.as_ref()) {
        process.env("CLAUDE_CONFIG_DIR", config_dir);
    }
    crate::cli_path::reject_windows_batch_cli(cli_path.as_os_str())?;
    let mut child = process_provider.spawn(&mut process).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            match options.cwd.as_ref() {
                Some(cwd) if !cwd.exists() => Error::WorkingDirectoryMissing(cwd.clone()),
                _ => Error::CliNotFoundAt(cli_path.clone()),
            }
        } else {
            Error::Spawn(error)
        }
    })?;
    let process_id = child.id();
    let stdin = child.stdin.take().ok_or(Error::PipeUnavailable)?;
    let stdout = child.stdout.take().ok_or(Error::PipeUnavailable)?;
    let stderr = child.stderr.take().ok_or(Error::PipeUnavailable)?;
    let stderr = tokio::spawn(drain_stderr_with(
        stderr,
        options.limits.max_stderr_bytes.get(),
        options.stderr_handler.clone(),
    ));
    let driver = Driver {
        child,
        stdin: Some(stdin),
        frames: FrameReader::new(stdout, options.limits.max_frame_bytes.get()),
        stderr,
        commands,
        messages,
        shutdown,
        limits: options.limits,
        handler: options.permission_handler,
        pending: HashMap::new(),
        callbacks: JoinSet::new(),
        callback_handles: HashMap::new(),
        callback_signals: HashMap::new(),
        hook_callbacks,
        sdk_mcp_servers: options
            .sdk_mcp_servers
            .iter()
            .map(|(name, server)| (name.clone(), server.open_session()))
            .collect(),
        pending_mcp: None,
        initialize_request,
        blocking_hooks,
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
        last_error_result: None,
        closed: closed.clone(),
        version_check: (!crate::version_check::skip_requested(
            inherited
                .get(std::ffi::OsStr::new("CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK"))
                .map(|v| v.as_os_str()),
        ))
        .then(|| cli_path.display().to_string()),
    };
    Ok((process_id, tokio::spawn(driver.run(terminal))))
}

impl<I, O, F> Driver<I, O, F>
where
    I: AsyncWrite + Unpin + Send + 'static,
    O: AsyncRead + Unpin + Send + 'static,
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
        self.stdin.take();
        let immediate = primary.is_err() && !matches!(primary, Err(Error::UnexpectedEof));
        let cleanup = self.reap(immediate).await;
        let exited = cleanup
            .as_ref()
            .ok()
            .map(|(status, _, _)| crate::error::exit_code(status));
        let mirror_cleanup = self.finalize_mirror().await;
        if let Some(materialized) = self.session.as_mut().and_then(|session| session.materialized.take()) {
            materialized.cleanup().await;
        }
        self.stderr.abort();
        let outcome = match cleanup {
            Ok((status, stderr, forced)) => match primary {
                Err(Error::UnexpectedEof) if !status.success() => Err(self.exit_error(status, stderr)),
                Err(error) => Err(error),
                Ok(()) if !forced && !status.success() => Err(self.exit_error(status, stderr)),
                Ok(()) => Ok(()),
            },
            Err(cleanup) => match primary {
                Ok(()) => Err(cleanup),
                Err(primary) => Err(Error::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                }),
            },
        };
        if let Some(exit_code) = exited {
            self.closed.record(match self.argv_style {
                crate::ArgvStyle::Python => ClosedReason::Terminated { exit_code },
                _ => ClosedReason::NotReady,
            });
        }
        let outcome = combine_outcomes(outcome, mirror_cleanup);
        for (_, pending) in self.pending.drain() {
            pending.response.send_error(Error::DriverStopped);
        }
        self.commands.close();
        drop(self.messages);
        let _ = terminal.send(outcome);
    }

    /// Decodes one stdout line. TypeScript skips every line that is not JSON; Python fails on a corrupt line with
    /// `CLIJSONDecodeError` but drops a corrupt unterminated tail at end of file. A JSON line that is not a valid
    /// message is a `MessageParseError` carrying the raw object in both.
    fn decode_frame(&self, frame: &[u8]) -> Result<Option<Frame>, Error> {
        let error = match wire::parse_frame(frame) {
            Ok(parsed) => return Ok(Some(parsed)),
            Err(error) => error,
        };
        if let ProtocolError::Json(json) = &error
            && matches!(
                json.classify(),
                serde_json::error::Category::Syntax | serde_json::error::Category::Eof
            )
        {
            if self.argv_style == crate::ArgvStyle::TypeScript || self.frames.last_was_tail() {
                tracing::debug!("skipping undecodable line from CLI stdout");
                return Ok(None);
            }
            let ProtocolError::Json(json) = error else {
                return Err(error.into());
            };
            return Err(crate::JsonDecodeError::new(frame, json).into());
        }
        let data = match serde_json::from_slice::<serde_json::Value>(frame) {
            Ok(serde_json::Value::Object(object)) => Some(object),
            _ => None,
        };
        Err(crate::MessageParseError::new(error, data).into())
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
                self.stdin.take();
            }
            let between_turns = self.run_state.input_done.is_some()
                && self.run_state.result_received
                && !self.run_state.turn_active
                && self.callback_handles.is_empty()
                && self.run_state.tasks.is_empty()
                && !matches!(self.run_state.session_state, Some(SessionState::RequiresAction));
            if between_turns && self.stdin.is_some() {
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
                frame = self.frames.next() => {
                    let Some(frame) = frame? else {
                        self.finish_mirror().await?;
                        return if matches!(self.state, State::Running | State::Initializing) || !self.pending.is_empty() { Err(Error::UnexpectedEof) } else { Ok(()) };
                    };
                    if !frame.trim_ascii_start().starts_with(b"{") {
                        if !frame.trim_ascii().is_empty() {
                            tracing::debug!("skipping non-JSON line from CLI stdout");
                        }
                        continue;
                    }
                    let Some(parsed) = self.decode_frame(&frame)? else { continue; };
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
                    self.stdin.take();
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
                if self.stdin.is_none() {
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
                self.next_id = self.next_id.checked_add(1).ok_or(Error::RequestIdExhausted)?;
                let id = ControlRequestId::from(format!("request-{}", self.next_id));
                let frame = wire::control_request(&id, RequestPayload::McpSetServers { servers }.to_value()?)?;
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
                if self.stdin.is_none() {
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

    async fn start_control(&mut self, response: ControlResponder, request: serde_json::Value) -> Result<(), Error> {
        if self.stdin.is_none() {
            response.send_error(Error::NotReadyForWriting);
            return Ok(());
        }
        if self.pending.len() >= self.limits.max_pending_controls.get() {
            response.send_error(Error::TooManyControls);
            return Ok(());
        }
        self.next_id = self.next_id.checked_add(1).ok_or(Error::RequestIdExhausted)?;
        let id = ControlRequestId::from(format!("request-{}", self.next_id));
        self.write(wire::control_request(&id, request)?).await?;
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
                            ControlResponder::Initialize(sender) => {
                                let connection_info =
                                    crate::ConnectionInfo::from_json(response).map_err(ProtocolError::from)?;
                                if self.blocking_hooks
                                    && connection_info.hook_on_failure_supported != crate::FeatureSupport::Supported
                                {
                                    let _ = sender.send(Err(Error::BlockingHooksUnsupported));
                                    return Err(Error::BlockingHooksUnsupported);
                                }
                                if self.blocking_hooks
                                    && connection_info.hooks_applied == crate::FeatureSupport::Unsupported
                                {
                                    let _ = sender.send(Err(Error::HooksNotApplied));
                                    return Err(Error::HooksNotApplied);
                                }
                                self.state = State::Idle;
                                let _ = sender.send(Ok(connection_info));
                            }
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
        if self.stdin.is_none() {
            // The run has settled and stdin is closed: a late callback answer has nowhere
            // to go, and that must not stop the driver.
            tracing::debug!("dropping a control response written after stdin closed");
            return Ok(());
        }
        self.write(frame).await
    }

    async fn write(&mut self, frame: serde_json::Value) -> Result<(), Error> {
        let mut bytes = serde_json::to_vec(&frame).map_err(ProtocolError::from)?;
        bytes.push(b'\n');
        if let Some(error) = self.write_unavailable() {
            return Err(error);
        }
        let stdin = self.stdin.as_mut().ok_or(Error::NotReadyForWriting)?;
        tokio::select! {
            _ = &mut self.shutdown => Err(Error::ShutdownRequested),
            result = timeout(self.limits.control_timeout.as_duration(), stdin.write_all(&bytes)) => result.map_err(|_| Error::WriteTimeout)?.map_err(Error::WriteFailed),
        }
    }

    /// Why a write cannot go out, in upstream's order: not ready, then terminated. Python keeps `_ready` after the
    /// process exits and reports the terminated process with its exit code. TypeScript's `exit` event clears
    /// `ready`, so a write after an observed exit is "not ready"; its `exited with error` branch is only reachable
    /// while the process is still running with an exit error recorded, a state neither SDK can enter (Python sets
    /// `_exit_error` after `returncode`, TypeScript clears `ready` with the error).
    fn write_unavailable(&mut self) -> Option<Error> {
        if self.stdin.is_none() {
            return Some(Error::NotReadyForWriting);
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            return Some(match self.argv_style {
                crate::ArgvStyle::Python => Error::WriteToTerminatedProcess {
                    exit_code: crate::error::exit_code(&status),
                },
                _ => Error::NotReadyForWriting,
            });
        }
        None
    }

    /// SIGTERM on Unix, a hard kill where there is no such signal or it cannot be delivered.
    async fn terminate(&mut self) -> Result<(), Error> {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            let delivered = tokio::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .is_ok_and(|status| status.success());
            if delivered {
                return Ok(());
            }
        }
        self.child.start_kill().map_err(Error::Io)
    }

    async fn reap(&mut self, immediate: bool) -> Result<(std::process::ExitStatus, StderrTail, bool), Error> {
        // Python's close sequence: stdin is already closed, wait, SIGTERM, wait, SIGKILL, wait.
        let wait = self.limits.shutdown_timeout.as_duration();
        let mut forced = immediate;
        if immediate && self.child.try_wait().map_err(Error::Io)?.is_none() {
            self.terminate().await?;
        }
        let status = match timeout(wait, self.child.wait()).await {
            Ok(status) => status.map_err(Error::Io)?,
            Err(_) => {
                forced = true;
                self.terminate().await?;
                match timeout(wait, self.child.wait()).await {
                    Ok(status) => status.map_err(Error::Io)?,
                    Err(_) => {
                        self.child.start_kill().map_err(Error::Io)?;
                        timeout(wait, self.child.wait())
                            .await
                            .map_err(|_| Error::ShutdownTimeout)?
                            .map_err(Error::Io)?
                    }
                }
            }
        };
        let stderr = match timeout(self.limits.shutdown_timeout.as_duration(), &mut self.stderr).await {
            Ok(result) => result.map_err(Error::DriverTask)?.map_err(Error::Io)?,
            Err(_) => {
                self.stderr.abort();
                return Err(Error::StderrDrainTimeout);
            }
        };
        Ok((status, stderr, forced))
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
