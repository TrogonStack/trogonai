use futures_util::StreamExt;

use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

use crate::{
    ClaudeAgentOptions, ControlOperation, ControlReply, Error, JsonObject, Message, Prompt, ProtocolError,
    driver::{self, Command, Completion},
    js_runtime::Nullish,
    process_options::EnvironmentMode,
    process_transport::{Closed, ProcessSpawn, ProcessTransport},
    python_c_stack::PythonCStack,
    startup_timeout::{StartupEvent, StartupTimer},
    upstream::control as wire_types,
    wire::runtime_payload::RequestPayload,
};

mod constants;

use constants::CLAIM_FLAGS;

struct StartInput<T, F: crate::adapters::SessionFileSystem> {
    options: ClaudeAgentOptions,
    session: Option<crate::SessionRuntime<F>>,
    transport: T,
    initialization: driver::Initialization,
    version_check: Option<String>,
    closed: Closed,
    process_id: Option<u32>,
    /// `startup()` and `prewarm()`: the initialize timer and the frames it follows.
    startup: Option<crate::startup_timeout::StartupWatch>,
    startup_tap: Option<tokio::sync::mpsc::UnboundedSender<crate::startup_timeout::StartupEvent>>,
}

/// Owns one Claude Code subprocess. Keep consuming messages while a turn runs.
pub struct ClaudeSdkClient {
    runtime: ClientRuntime,
}

struct ClientRuntime {
    handle: ClientHandle,
    messages: crate::channel::Receiver<Message>,
    terminal: Option<oneshot::Receiver<Result<(), Error>>>,
    driver: Option<JoinHandle<()>>,
    process_id: Option<u32>,
    /// What the terminal reported, kept while `finish` still waits for the driver task.
    outcome: Option<Result<(), Error>>,
}

/// What the first initialize came to: its response, or the error it failed with, shared by every reader the way a
/// settled `sdk.mjs` promise is.
type InitializationOutcome = Result<Option<serde_json::Value>, std::sync::Arc<Error>>;

/// A cloneable command handle that does not extend the subprocess owner's life.
#[derive(Clone)]
pub struct ClientHandle {
    commands: crate::channel::Sender<Command>,
    /// Only a bounded command queue can be full, so the wait applies only when the caller opted into both.
    timeout: Option<std::time::Duration>,
    abort: crate::CancellationToken,
    close: CloseSignal,
    closed: Closed,
    /// The first initialize response, which `reinitialize` does not replace; `None` inside when the reply had none.
    initialization: std::sync::Arc<tokio::sync::watch::Sender<Option<InitializationOutcome>>>,
    latest_commands: driver::LatestCommands,
    prompt_hold: driver::PromptHold,
    host: crate::remote_control::HostState,
    style: crate::ArgvStyle,
    /// Words the `TypeError`s the TypeScript getters throw on a missing initialization result.
    js_runtime: crate::JsRuntime,
    /// `true` while a `claim_session` request of a spare is unanswered.
    claim_pending: std::sync::Arc<tokio::sync::watch::Sender<bool>>,
}

/// A control request that is queued for writing and not yet answered.
pub(crate) struct ControlTicket {
    handle: ClientHandle,
    receiver: oneshot::Receiver<Result<Option<serde_json::Value>, Error>>,
}

impl ControlTicket {
    /// The answer's `response` field, `None` when the frame has none.
    pub(crate) async fn wait(self) -> Result<Option<serde_json::Value>, Error> {
        self.receiver.await.map_err(|_| self.handle.stopped_error())?
    }
}

/// TypeScript `Query.close()`: asks the driver to shut down, once, from the owner or any handle.
#[derive(Clone)]
struct CloseSignal(std::sync::Arc<std::sync::Mutex<Option<oneshot::Sender<()>>>>);

impl CloseSignal {
    fn new(sender: oneshot::Sender<()>) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(sender))))
    }

    /// `true` once `close` ran: TypeScript `cleanupPerformed`, Python `_closed`.
    fn fired(&self) -> bool {
        match self.0.lock() {
            Ok(sender) => sender.is_none(),
            Err(poisoned) => poisoned.into_inner().is_none(),
        }
    }

    fn fire(&self) {
        let sender = match self.0.lock() {
            Ok(mut sender) => sender.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(sender) = sender {
            let _ = sender.send(());
        }
    }
}

/// The inherited variables plus the two the default debug file needs. They are read only here, and the
/// configuration directory only when the debug variable is set, so an ordinary connection stays unobserved.
fn debug_probe<E: crate::adapters::EnvironmentVariable>(
    environment: &E,
    inherited: &crate::environment_plan::Inherited,
) -> crate::environment_plan::Inherited {
    let mut probe = inherited.clone();
    let debug = std::ffi::OsString::from(crate::constants::SDK_DEBUG_ENV);
    if let Some(value) = environment.var_os(&debug) {
        probe.insert(debug, value);
        let config = std::ffi::OsString::from("CLAUDE_CONFIG_DIR");
        if let Some(value) = environment.var_os(&config) {
            probe.insert(config, value);
        }
    }
    probe
}

/// TypeScript `OD`: the startup marks variable is set unless the environment the CLI starts with already has it,
/// matched without regard to case. Replace mode, the TypeScript default when `options.env` is set, starts from
/// `options.env` alone.
fn startup_marks_requested<E: crate::adapters::EnvironmentVariable>(
    options: &ClaudeAgentOptions,
    environment: &E,
) -> bool {
    const NAME: &str = "CLAUDE_CODE_SDK_READS_STARTUP_MARKS";
    let mode = options.environment_mode.resolve(&options.env);
    (mode != EnvironmentMode::Inherit && options.env.iter().any(|(key, _)| key.eq_ignore_ascii_case(NAME)))
        || (mode != EnvironmentMode::Replace
            && environment
                .vars_os()
                .iter()
                .any(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(NAME)))
}

fn capture_inherited<E: crate::adapters::EnvironmentVariable>(
    environment: &E,
    options: &ClaudeAgentOptions,
) -> crate::environment_plan::Inherited {
    let mut inherited: crate::environment_plan::Inherited = crate::constants::INHERITED_KEYS
        .iter()
        .filter_map(|key| {
            let key = std::ffi::OsString::from(*key);
            environment.var_os(&key).map(|value| (key, value))
        })
        .collect();
    // Only a derivation that checks for an explicit label looks at the inherited one.
    if options.client_app_derivation != crate::ClientAppDerivation::Disabled {
        let key = std::ffi::OsString::from(crate::constants::CLIENT_APP_ENV);
        if let Some(value) = environment.var_os(&key) {
            inherited.insert(key, value);
        }
    }
    // Upstream matches the session-state variable case-insensitively across the whole inherited environment.
    // The `toolConfig` variables are removed from an inherited environment in any letter case.
    inherited.extend(environment.vars_os().into_iter().filter(|(key, _)| {
        let key = key.to_string_lossy();
        key.eq_ignore_ascii_case("CLAUDE_CODE_SDK_READS_SESSION_STATE")
            || crate::constants::TOOL_CONFIG_ENVS
                .iter()
                .any(|name| key.eq_ignore_ascii_case(name))
    }));
    inherited
}

impl ClaudeSdkClient {
    /// Connects with the native process spawner. Like Python's `SubprocessCLITransport`, an omitted
    /// `cli_path` is discovered here; `connect_with_adapters` never discovers because its caller owns spawning
    /// (upstream skips discovery for a custom transport and for TypeScript's `spawnClaudeCodeProcess`).
    pub async fn connect(mut options: ClaudeAgentOptions) -> Result<Self, Error> {
        let adapters: crate::adapters::RuntimeAdapters = crate::adapters::RuntimeAdapters::default();
        let discovered = if options.cli_path.is_none() {
            let inherited = capture_inherited(&adapters.environment, &options);
            crate::argv::request_await_initialize(&mut options);
            Some(crate::cli_discovery::discover(
                &adapters.environment,
                &inherited,
                &adapters.filesystem,
                cfg!(windows),
                options.bundled_cli_root.as_deref(),
            )?)
        } else {
            None
        };
        Box::pin(Self::connect_resolved(
            options,
            adapters,
            discovered,
            true,
            None,
            PythonCStack::CLIENT_CONNECT,
        ))
        .await
    }

    /// Connects through caller-supplied adapters. The caller owns spawning, so no executable discovery runs and
    /// an omitted `cli_path` reaches the spawner as `claude`.
    pub async fn connect_with_adapters<E, P, F>(
        options: ClaudeAgentOptions,
        adapters: crate::adapters::RuntimeAdapters<E, P, F>,
    ) -> Result<Self, Error>
    where
        E: crate::adapters::EnvironmentVariable
            + crate::adapters::CurrentDirectory<Error = std::io::Error>
            + crate::adapters::TemporaryDirectory,
        P: crate::adapters::SpawnProcess<Error = std::io::Error>,
        F: crate::adapters::SessionFileSystem,
    {
        Box::pin(Self::connect_resolved(
            options,
            adapters,
            None,
            false,
            None,
            PythonCStack::CLIENT_CONNECT,
        ))
        .await
    }

    /// `startup()` and `prewarm()`: connects, then waits for initialize under the startup timer. Like `connect`, an
    /// omitted `cli_path` is discovered when the native spawner is used.
    pub(crate) async fn connect_startup<E, P, F>(
        mut options: ClaudeAgentOptions,
        adapters: crate::adapters::RuntimeAdapters<E, P, F>,
        discover: bool,
        startup: crate::startup_timeout::StartupSettings,
    ) -> Result<Self, Error>
    where
        E: crate::adapters::EnvironmentVariable
            + crate::adapters::CurrentDirectory<Error = std::io::Error>
            + crate::adapters::TemporaryDirectory,
        P: crate::adapters::SpawnProcess<Error = std::io::Error>,
        F: crate::adapters::SessionFileSystem,
    {
        let discovered = if discover && options.cli_path.is_none() {
            let inherited = capture_inherited(&adapters.environment, &options);
            crate::argv::request_await_initialize(&mut options);
            Some(crate::cli_discovery::discover(
                &adapters.environment,
                &inherited,
                &adapters.filesystem,
                cfg!(windows),
                options.bundled_cli_root.as_deref(),
            )?)
        } else {
            None
        };
        Box::pin(Self::connect_resolved(
            options,
            adapters,
            discovered,
            discover,
            Some(startup),
            // Python 0.2.181 has no startup() or prewarm(), and builds the command at connect time.
            PythonCStack::CLIENT_CONNECT,
        ))
        .await
    }

    async fn connect_resolved<E, P, F>(
        mut options: ClaudeAgentOptions,
        adapters: crate::adapters::RuntimeAdapters<E, P, F>,
        discovered: Option<std::path::PathBuf>,
        derive_debug_file: bool,
        startup: Option<crate::startup_timeout::StartupSettings>,
        python_stack: PythonCStack,
    ) -> Result<Self, Error>
    where
        E: crate::adapters::EnvironmentVariable
            + crate::adapters::CurrentDirectory<Error = std::io::Error>
            + crate::adapters::TemporaryDirectory,
        P: crate::adapters::SpawnProcess<Error = std::io::Error>,
        F: crate::adapters::SessionFileSystem,
    {
        let inherited = capture_inherited(&adapters.environment, &options);
        let cli_path = options
            .cli_path
            .clone()
            .or(discovered)
            .unwrap_or_else(|| "claude".into());
        let session = Box::pin(Self::prepare_session(
            &mut options,
            &adapters.environment,
            &adapters.process,
            adapters.filesystem,
            true,
        ))
        .await?;
        crate::shadow_warning::emit(&options);
        let initialization = driver::Initialization::new(&options, &adapters.environment)?;
        let closed = Closed::default();
        // The transport forwards this controller to a `spawn_claude_code_process` process, so it is created here.
        options.abort_controller.get_or_insert_default();
        // TypeScript derives a default debug file only when the SDK spawns the CLI and none was given.
        let derived_debug_file = if derive_debug_file
            && options.argv_style == crate::ArgvStyle::TypeScript
            && options.spawn_claude_code_process.is_none()
            && options
                .debug_file
                .as_ref()
                .is_none_or(|file| file.as_os_str().is_empty())
        {
            let probe = debug_probe(&adapters.environment, &inherited);
            crate::debug_file::process_debug_file(&adapters.environment, &probe)?
        } else {
            None
        };
        let transport = ProcessTransport::spawn(ProcessSpawn {
            options: &options,
            cli_path: cli_path.clone(),
            inherited: &inherited,
            process: &adapters.process,
            config_dir: session.as_ref().and_then(|session| session.config_dir.as_deref()),
            derived_debug_file: derived_debug_file.as_deref(),
            secure_storage_config_dir: session.as_ref().and_then(|session| {
                crate::session_resume::secure_storage_config_dir(
                    session.materialized.is_some(),
                    options.session_store.as_ref().map(|store| store.dialect),
                    &options.env,
                    &inherited,
                    cfg!(windows),
                )
            }),
            closed: closed.clone(),
            parent_environment: if options.spawn_claude_code_process.is_some() {
                adapters.environment.vars_os()
            } else {
                Vec::new()
            },
            startup_marks: startup.is_some() && !startup_marks_requested(&options, &adapters.environment),
            python_stack,
        })?;
        let version_check = (!crate::version_check::skip_requested(
            inherited
                .get(std::ffi::OsStr::new("CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK"))
                .map(|v| v.as_os_str()),
        ))
        .then(|| cli_path.display().to_string());
        let process_id = transport.process_id();
        let (startup, startup_tap) = match startup {
            Some(settings) => {
                let (tap, events) = tokio::sync::mpsc::unbounded_channel();
                (
                    Some(crate::startup_timeout::StartupWatch { settings, events }),
                    Some(tap),
                )
            }
            None => (None, None),
        };
        Box::pin(Self::start(StartInput {
            options,
            session,
            transport,
            initialization,
            version_check,
            closed,
            process_id,
            startup,
            startup_tap,
        }))
        .await
    }

    /// Connects through a caller-supplied [`Transport`], Python's `ClaudeSDKClient(transport=...)`.
    ///
    /// Like upstream, the SDK does not discover or spawn a CLI, does not build its argument list or environment,
    /// and does not materialize a store-backed resume for a custom transport. A configured session store still
    /// mirrors the transcript frames the transport yields.
    pub async fn connect_with_transport<T: crate::Transport>(
        options: ClaudeAgentOptions,
        transport: T,
    ) -> Result<Self, Error> {
        let adapters: crate::adapters::RuntimeAdapters = crate::adapters::RuntimeAdapters::default();
        Box::pin(Self::connect_with_transport_and_adapters(options, transport, adapters)).await
    }

    /// [`ClaudeSdkClient::connect_with_transport`] with caller-supplied adapters for the session store mirror.
    pub async fn connect_with_transport_and_adapters<T, E, P, F>(
        options: ClaudeAgentOptions,
        transport: T,
        adapters: crate::adapters::RuntimeAdapters<E, P, F>,
    ) -> Result<Self, Error>
    where
        T: crate::Transport,
        E: crate::adapters::EnvironmentVariable
            + crate::adapters::CurrentDirectory<Error = std::io::Error>
            + crate::adapters::TemporaryDirectory,
        P: crate::adapters::SpawnProcess<Error = std::io::Error>,
        F: crate::adapters::SessionFileSystem,
    {
        Box::pin(Self::connect_declaring(options, transport, adapters, false)).await
    }

    /// Connects the browser query: like [`ClaudeSdkClient::connect_with_transport`], with `declare_dialog_kinds`
    /// sending `supportedDialogKinds: []` the way an empty list does upstream.
    pub(crate) async fn connect_browser<T: crate::Transport>(
        options: ClaudeAgentOptions,
        transport: T,
        declare_dialog_kinds: bool,
    ) -> Result<Self, Error> {
        let adapters: crate::adapters::RuntimeAdapters = crate::adapters::RuntimeAdapters::default();
        Box::pin(Self::connect_declaring(
            options,
            transport,
            adapters,
            declare_dialog_kinds,
        ))
        .await
    }

    async fn connect_declaring<T, E, P, F>(
        mut options: ClaudeAgentOptions,
        transport: T,
        adapters: crate::adapters::RuntimeAdapters<E, P, F>,
        declare_dialog_kinds: bool,
    ) -> Result<Self, Error>
    where
        T: crate::Transport,
        E: crate::adapters::EnvironmentVariable
            + crate::adapters::CurrentDirectory<Error = std::io::Error>
            + crate::adapters::TemporaryDirectory,
        P: crate::adapters::SpawnProcess<Error = std::io::Error>,
        F: crate::adapters::SessionFileSystem,
    {
        let session = Box::pin(Self::prepare_session(
            &mut options,
            &adapters.environment,
            &adapters.process,
            adapters.filesystem,
            false,
        ))
        .await?;
        crate::shadow_warning::emit(&options);
        let mut initialization = driver::Initialization::new(&options, &adapters.environment)?;
        if declare_dialog_kinds {
            initialization.declare_dialog_kinds();
        }
        Box::pin(Self::start(StartInput {
            options,
            session,
            transport,
            initialization,
            version_check: None,
            closed: Closed::default(),
            process_id: None,
            startup: None,
            startup_tap: None,
        }))
        .await
    }

    /// Python `validate_session_store_options` plus `materialize_resume_session`. The materialization is skipped
    /// for a custom transport, because the materialized options never reach it.
    async fn prepare_session<E, P, F>(
        options: &mut ClaudeAgentOptions,
        environment: &E,
        process: &P,
        filesystem: F,
        materialize: bool,
    ) -> Result<Option<crate::SessionRuntime<F>>, Error>
    where
        E: crate::adapters::EnvironmentVariable
            + crate::adapters::CurrentDirectory<Error = std::io::Error>
            + crate::adapters::TemporaryDirectory,
        P: crate::adapters::SpawnProcess<Error = std::io::Error>,
        F: crate::adapters::SessionFileSystem,
    {
        let Some(store_options) = &options.session_store else {
            return Ok(None);
        };
        if options.argv_style == crate::ArgvStyle::TypeScript
            && options.session_persistence == crate::SessionPersistence::Disabled
        {
            return Err(Error::SessionStoreWithoutPersistence);
        }
        let mut store_options = store_options.clone();
        if let Some((_, directory)) = options
            .env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        {
            let directory = std::path::PathBuf::from(directory.expose());
            if store_options
                .config_directory
                .as_ref()
                .is_some_and(|configured| configured != &directory)
            {
                tracing::warn!(
                    "the subprocess CLAUDE_CONFIG_DIR differs from the session store configuration directory; using the subprocess value so transcript mirror frames are not dropped"
                );
            }
            store_options.config_directory = Some(directory);
        }
        let resume = match &options.resume {
            _ if !materialize => crate::StoreResume::None,
            Some(session) => crate::StoreResume::Session(session.clone()),
            None if options.continue_conversation == crate::ContinueConversation::Yes => {
                if !store_options.store.supports(crate::SessionStoreOperation::List) {
                    return Err(Error::ContinueRequiresListSessions);
                }
                crate::StoreResume::Latest
            }
            None => crate::StoreResume::None,
        };
        // The project key comes from `cwd ?? process.cwd()` in both upstream SDKs, which never hand that directory
        // to the spawn: the child keeps `options.cwd`, and with none it inherits the working directory and gets no
        // `PWD`.
        let cwd = match options.cwd.clone() {
            Some(cwd) => cwd,
            None => environment.current_dir().map_err(Error::Io)?,
        };
        let runtime = crate::SessionRuntime::prepare_with_adapters(crate::SessionPreparationOptions {
            store_options: &store_options,
            resume,
            cwd: &cwd,
            subprocess_environment: &options.env,
            environment_mode: options.environment_mode,
            environment,
            process,
            filesystem,
        })
        .await?;
        if let Some(session_id) = &runtime.resume_session_id {
            options.resume = Some(session_id.clone());
            options.continue_conversation = crate::ContinueConversation::No;
        }
        Ok(Some(runtime))
    }

    async fn start<T: crate::Transport, F: crate::adapters::SessionFileSystem>(
        input: StartInput<T, F>,
    ) -> Result<Self, Error> {
        let StartInput {
            options,
            session,
            transport,
            initialization,
            version_check,
            closed,
            process_id,
            startup,
            startup_tap,
        } = input;
        let (command_tx, command_rx) = crate::channel::channel(options.limits.command_queue_capacity);
        let (message_tx, message_rx) = crate::channel::channel(options.limits.message_buffer.capacity());
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let abort = options.abort_controller.clone().unwrap_or_default();
        let handle = ClientHandle {
            commands: command_tx,
            timeout: options
                .limits
                .command_queue_capacity
                .and(options.limits.command_queue_timeout)
                .map(crate::Timeout::as_duration),
            abort: abort.clone(),
            close: CloseSignal::new(shutdown_tx),
            closed,
            initialization: std::sync::Arc::new(tokio::sync::watch::channel(None).0),
            latest_commands: driver::LatestCommands::default(),
            prompt_hold: driver::PromptHold::default(),
            host: crate::remote_control::HostState::default(),
            style: options.argv_style,
            js_runtime: options.js_runtime,
            claim_pending: std::sync::Arc::new(tokio::sync::watch::channel(false).0),
        };
        let latest_commands = handle.latest_commands.clone();
        let host = handle.host.clone();
        let materialized = session.as_ref().is_some_and(|session| session.materialized.is_some());
        let (first_initialize, receiver) = oneshot::channel();
        let driver = driver::spawn(driver::SpawnOptions {
            options,
            session,
            commands: command_rx,
            messages: message_tx,
            shutdown: shutdown_rx,
            abort,
            terminal: terminal_tx,
            transport,
            initialization,
            version_check,
            latest_commands,
            prompt_hold: handle.prompt_hold.clone(),
            host,
            startup_tap,
            first_initialize,
        })
        .await?;
        let runtime = ClientRuntime {
            handle,
            messages: message_rx,
            terminal: Some(terminal_rx),
            driver: Some(driver),
            process_id,
            outcome: None,
        };
        // TypeScript: a process spawned under an aborted signal never hands back a Query; it ends with "Claude Code
        // process aborted by user".
        if runtime.handle.abort.is_cancelled() {
            return Err(Self::failed_start(runtime, crate::AbortError::PROCESS.into()).await);
        }
        // `startup()` and `prewarm()` wait for initialize under the startup timer, whichever dialect.
        if let Some(watch) = startup {
            return match runtime.handle.initialize_watched(receiver, watch).await {
                Ok(response) => {
                    runtime.handle.initialization.send_replace(Some(Ok(response)));
                    Ok(Self { runtime })
                }
                Err(error) => Err(Self::failed_startup(runtime, error, materialized).await),
            };
        }
        match runtime.handle.style {
            // `sdk.mjs` hands back the Query as soon as the initialize request is written. Whatever the caller
            // sends next is written behind it, and a failed initialize surfaces where the initialization result
            // is read, and through the run when something depended on it.
            crate::ArgvStyle::TypeScript => {
                let outcome = runtime.handle.initialization.clone();
                tokio::spawn(async move {
                    let result = match receiver.await {
                        Ok(result) => result,
                        Err(_) => Err(Error::DriverStopped),
                    };
                    outcome.send_replace(Some(result.map_err(std::sync::Arc::new)));
                });
                Ok(Self { runtime })
            }
            // Python `connect()` and `query()` wait for the initialize response.
            crate::ArgvStyle::Python => {
                let initialized = receiver
                    .await
                    .map_err(|_| Error::DriverStopped)
                    .and_then(|response| response);
                match initialized {
                    Ok(response) => {
                        runtime.handle.initialization.send_replace(Some(Ok(response)));
                        Ok(Self { runtime })
                    }
                    Err(error) => Err(Self::failed_start(runtime, error).await),
                }
            }
        }
    }

    /// `startup()` rejecting. TypeScript `O7` calls `close()` without waiting and rethrows, and waits for the exit
    /// first only when a resumed session's configuration directory has to be removed, throwing the same error either
    /// way. An initialize the process ended by exiting still reads the run's outcome, which names why it ended.
    async fn failed_startup(mut runtime: ClientRuntime, error: Error, materialized: bool) -> Error {
        if runtime.handle.abort.is_cancelled() || matches!(error, Error::DriverStopped) {
            return Self::failed_start(runtime, error).await;
        }
        runtime.handle.close();
        if materialized && let Err(cleanup) = runtime.finish().await {
            tracing::debug!(error = %cleanup, "Claude Code cleanup after a failed startup");
        }
        error
    }

    async fn failed_start(mut runtime: ClientRuntime, error: Error) -> Error {
        // TypeScript: a process spawned under an aborted signal ends with "Claude Code process aborted by user".
        if runtime.handle.abort.is_cancelled() {
            return runtime.finish().await.err().unwrap_or(error);
        }
        let cleanup = if matches!(error, Error::DriverStopped) {
            runtime.finish().await
        } else {
            runtime.close().await
        };
        connection_failure(error, cleanup)
    }

    pub fn handle(&self) -> ClientHandle {
        self.runtime.handle.clone()
    }

    pub fn process_id(&self) -> Option<u32> {
        self.runtime.process_id
    }

    pub async fn send_prompt(&self, prompt: Prompt) -> Result<(), Error> {
        self.send_input(prompt.into()).await
    }

    pub async fn send_input(&self, input: crate::UserInput) -> Result<(), Error> {
        self.runtime.handle.send_input(input).await
    }

    pub async fn finish_input(&self) -> Result<(), Error> {
        self.runtime.handle.finish_input().await
    }

    /// Stops the CLI process immediately; the run then ends with `Error::Aborted` (TypeScript `AbortError`).
    pub async fn abort(&self) -> Result<(), Error> {
        self.runtime.handle.abort().await
    }

    /// TypeScript `Query.close()`: shuts the process down without waiting. Idempotent. Messages already received
    /// stay readable, after which `next_message` returns `None`; control requests still waiting fail with
    /// `Error::QueryClosed`. `disconnect` closes and also waits for cleanup.
    pub fn close(&self) {
        self.runtime.handle.close();
    }

    /// See [`ClientHandle::stream_input`].
    pub async fn stream_input<S>(&self, stream: S) -> Result<(), Error>
    where
        S: futures_util::Stream<Item = crate::UserInput> + Unpin,
    {
        self.runtime.handle.stream_input(stream).await
    }

    /// See [`ClientHandle::try_stream_input`].
    pub async fn try_stream_input<S, E>(&self, stream: S) -> Result<(), E>
    where
        S: futures_util::Stream<Item = Result<crate::UserInput, E>> + Unpin,
        E: From<Error> + std::fmt::Display,
    {
        self.runtime.handle.try_stream_input(stream).await
    }

    /// Python `ClaudeSDKClient.query(prompt)`: writes the prompt in session `"default"` under the Python style, or
    /// in the empty session `sdk.mjs` writes under the TypeScript style. See `query_in_session`.
    pub async fn query(&self, prompt: Prompt) -> Result<(), Error> {
        self.query_in_session(prompt, connect::default_session(self.runtime.handle.style))
            .await
    }

    /// Cancellation-safe: cancelling this wait does not discard a message.
    pub async fn next_message(&mut self) -> Result<Option<Message>, Error> {
        std::future::poll_fn(|cx| self.poll_next_message(cx)).await
    }

    /// Waits for process shutdown and final storage work, reporting cleanup errors.
    pub async fn disconnect(self) -> Result<(), Error> {
        self.runtime.close().await
    }
}

fn connection_failure(primary: Error, cleanup: Result<(), Error>) -> Error {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) if matches!(primary, Error::DriverStopped) => cleanup,
        Err(Error::DriverStopped) => primary,
        Err(Error::BlockingHooksUnsupported) if matches!(primary, Error::BlockingHooksUnsupported) => primary,
        Err(Error::HooksNotApplied) if matches!(primary, Error::HooksNotApplied) => primary,
        Err(Error::ManagedSettingsNotApplied) if matches!(primary, Error::ManagedSettingsNotApplied) => primary,
        Err(Error::ControlTimeout { operation }) if matches!(&primary, Error::ControlTimeout { operation: original } if *original == operation) => {
            primary
        }
        Err(cleanup) => Error::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        },
    }
}

impl ClientRuntime {
    async fn close(mut self) -> Result<(), Error> {
        self.handle.close();
        self.finish().await
    }

    async fn finish(&mut self) -> Result<(), Error> {
        std::future::poll_fn(|cx| self.poll_finish(cx)).await
    }
}

impl Drop for ClientRuntime {
    fn drop(&mut self) {
        self.handle.close();
    }
}

impl ClientHandle {
    async fn initialize(&self, first: bool) -> Result<Option<serde_json::Value>, Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::Initialize { first, response }).await?;
        receiver.await.map_err(|_| Error::DriverStopped)?
    }

    /// TypeScript `initializationResult()`, Python `get_server_info()`: the first initialize response. TypeScript
    /// hands the client out before the response arrives, so this waits for it; a failed initialize fails here with
    /// `Error::InitializationFailed`.
    pub async fn initialization_result(&self) -> Result<crate::ConnectionInfo, Error> {
        self.initialization_value()
            .await
            .map(crate::ConnectionInfo::from_response)
    }

    async fn initialization_value(&self) -> Result<Option<serde_json::Value>, Error> {
        self.wait_for_claim().await;
        let mut receiver = self.initialization.subscribe();
        loop {
            if let Some(outcome) = receiver.borrow_and_update().clone() {
                return outcome.map_err(Error::InitializationFailed);
            }
            if receiver.changed().await.is_err() {
                return Err(Error::DriverStopped);
            }
        }
    }

    /// TypeScript `claimSession`: the response's `initialize` replaces the initialization result, keeping the
    /// applied flags it omits or sends as `null`. A response without one leaves the result as it was.
    pub(crate) fn merge_claim_initialization(&self, response: &serde_json::Value) {
        let Some(update) = response.get("initialize") else {
            return;
        };
        let runtime = self.js_runtime;
        self.initialization.send_modify(|slot| {
            let merged = claim_initialization(slot.as_ref(), update, runtime);
            *slot = Some(merged);
        });
    }

    /// The initialize wait of `startup()`: TypeScript `m.race(initializationResult())`.
    async fn initialize_watched(
        &self,
        initialize: oneshot::Receiver<Result<Option<serde_json::Value>, Error>>,
        mut watch: crate::startup_timeout::StartupWatch,
    ) -> Result<Option<serde_json::Value>, Error> {
        let mut initialize = initialize;
        let mut timer = StartupTimer::new(
            watch.settings.initialize_timeout,
            tokio::time::Instant::now(),
            watch.settings.on_hooks_running.clone(),
        );
        let mut mark: Option<(String, tokio::time::Instant)> = None;
        let mut events_open = true;
        loop {
            let deadline = timer.deadline();
            tokio::select! {
                result = &mut initialize => return result.map_err(|_| Error::DriverStopped).and_then(|response| response),
                event = watch.events.recv(), if events_open => match event {
                    Some(StartupEvent::Mark { mark: name, at }) => mark = Some((name, at)),
                    Some(event) => timer.note(&event, tokio::time::Instant::now()),
                    None => events_open = false,
                },
                () = tokio::time::sleep_until(deadline) => {
                    let now = tokio::time::Instant::now();
                    if let Some(error) = timer.fire(now) {
                        // `startupMarkAgeMs` is how long before the timeout the last mark arrived. The timer is due
                        // at `deadline`; the runtime wakes this task at or after it, rounded up to its next tick,
                        // and that lateness is not part of the mark's age.
                        let age = mark.map(|(name, at)| {
                            let micros = deadline.saturating_duration_since(at).as_micros();
                            (name, u64::try_from((micros + 500) / 1000).unwrap_or(u64::MAX))
                        });
                        return Err(error.with_mark(age).into());
                    }
                }
            }
        }
    }

    /// A field of the first initialize response, `None` when it is missing (TypeScript `undefined`). TypeScript
    /// reads it with `(await this.initialization).models`, which throws the engine's `TypeError` when initialize
    /// answered without a response or with `null`.
    async fn initialization_field<T>(&self, key: &str) -> Result<ControlReply<T>, Error> {
        let value = self.initialization_value().await?;
        if let Some(nothing) = Nullish::of(value.as_ref()) {
            let expression = format!("(await this.initialization).{key}");
            return Err(self.js_runtime.read_error(nothing, key, &expression));
        }
        Ok(ControlReply::new(
            value.as_ref().and_then(|value| value.get(key)).cloned(),
        ))
    }

    /// TypeScript `reinitialize()`: sends the connect-time initialize request again, without SDK MCP server
    /// manifests, and returns its response. A response that fails the initialize checks ends the session.
    /// [`ClientHandle::initialization_result`] keeps the first response.
    pub async fn reinitialize(&self) -> Result<crate::ConnectionInfo, Error> {
        self.initialize(false).await.map(crate::ConnectionInfo::from_response)
    }

    /// TypeScript `supportedCommands()`: the list from the last `commands_changed` message, else the initialize
    /// one. It destructures the initialization result first, so a failed one, or an initialize answered without
    /// a response or with `null`, throws even after `commands_changed`.
    pub async fn supported_commands(&self) -> Result<ControlReply<Vec<wire_types::SlashCommand>>, Error> {
        let value = self.initialization_value().await?;
        if let Some(nothing) = Nullish::of(value.as_ref()) {
            return Err(self.js_runtime.destructure_error(nothing, "commands"));
        }
        let latest = self
            .latest_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Ok(ControlReply::new(latest.or_else(|| {
            value.as_ref().and_then(|value| value.get("commands")).cloned()
        })))
    }

    /// TypeScript `supportedModels()`.
    pub async fn supported_models(&self) -> Result<ControlReply<Vec<wire_types::ModelInfo>>, Error> {
        self.initialization_field("models").await
    }

    /// TypeScript `supportedAgents()`.
    pub async fn supported_agents(&self) -> Result<ControlReply<Vec<wire_types::AgentInfo>>, Error> {
        self.initialization_field("agents").await
    }

    /// TypeScript `accountInfo()`.
    pub async fn account_info(&self) -> Result<ControlReply<wire_types::AccountInfo>, Error> {
        self.initialization_field("account").await
    }

    /// TypeScript `setMcpServers`: the reply's `response` field is returned as sent, `None` when the reply has
    /// none, where TypeScript returns `undefined`. Under `ArgvStyle::Python` anything but an object reads as `{}`,
    /// as `claude_agent_sdk` control requests do.
    pub async fn set_mcp_servers(
        &self,
        external: crate::McpServers,
        embedded: crate::SdkMcpServers,
    ) -> Result<ControlReply<crate::McpSetServersResult>, Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::SetMcpServers {
            external,
            embedded,
            response,
        })
        .await?;
        let response = receiver.await.map_err(|_| Error::DriverStopped)??;
        Ok(ControlReply::new(response))
    }
    /// TypeScript `Query.streamInput` and Python `Query.stream_input`: writes every input of the stream, then ends
    /// the input. With nothing written, or nothing that needs the CLI (SDK MCP servers, hooks, a permission handler
    /// and the other callbacks), stdin closes at once; otherwise it closes when the run ends.
    ///
    /// TypeScript, the default: an abort stops the loop and the call returns `Ok`, as `AbortError` resolves the
    /// upstream promise. Behind `promptHold` (a blocking hook or managed settings waiting for initialize) the
    /// inputs queue without waiting, a close stops the loop, and a hold that fails returns `Ok` without ending
    /// the input. Otherwise a write after `close` fails with `Error::NotReadyForWriting`.
    ///
    /// Under `ArgvStyle::Python` a close stops the loop, and a failed write is logged and still ends the input;
    /// the call returns `Ok`, as Python swallows both.
    pub async fn stream_input<S>(&self, stream: S) -> Result<(), Error>
    where
        S: futures_util::Stream<Item = crate::UserInput> + Unpin,
    {
        self.try_stream_input(stream.map(Ok::<_, Error>)).await
    }

    /// [`stream_input`](Self::stream_input) over a stream that can fail, as an upstream async iterable can throw.
    /// TypeScript rejects with the stream's error and leaves stdin open. Python logs `Prompt stream failed;
    /// closing stdin` at error level, ends the input the usual way, and returns `Ok`.
    pub async fn try_stream_input<S, E>(&self, stream: S) -> Result<(), E>
    where
        S: futures_util::Stream<Item = Result<crate::UserInput, E>> + Unpin,
        E: From<Error> + std::fmt::Display,
    {
        match self.style {
            crate::ArgvStyle::Python => {
                self.python_stream_input(stream).await;
                Ok(())
            }
            crate::ArgvStyle::TypeScript => self.typescript_stream_input(stream).await,
        }
    }

    /// `sdk.mjs` `Query.streamInput`, after `query()` already waited for initialize to be written.
    async fn typescript_stream_input<S, E>(&self, mut stream: S) -> Result<(), E>
    where
        S: futures_util::Stream<Item = Result<crate::UserInput, E>> + Unpin,
        E: From<Error>,
    {
        let mut held = Vec::new();
        // `count`: every input taken from the stream, the one an abort stops at included.
        let mut count = 0;
        while let Some(item) = stream.next().await {
            let input = item?;
            count += 1;
            let hold = self.prompt_hold.state();
            // `promptHold !== undefined && cleanupPerformed`: a failed hold already ran `cleanup`.
            let cleaned_up = hold == driver::HoldState::Failed || self.close.fired();
            if self.abort.is_cancelled() || (hold != driver::HoldState::Clear && cleaned_up) {
                break;
            }
            if hold == driver::HoldState::Clear {
                if self.close.fired() {
                    return Err(self.closed.write_error().unwrap_or(Error::NotReadyForWriting).into());
                }
                match self.send_input(input).await {
                    Err(Error::Aborted(_)) => return Ok(()),
                    outcome => outcome?,
                }
                continue;
            }
            // `writeBehindPromptHold`: queued now, written once initialize is confirmed.
            let (response, receiver) = oneshot::channel();
            match self
                .enqueue(Command::Prompt {
                    input: Box::new(input),
                    response,
                })
                .await
            {
                Ok(()) => held.push(receiver),
                // The run stopped before the write was queued: the hold resolves `false`.
                Err(_) => return Ok(()),
            }
        }
        // `Promise.all` of the held writes and the hold itself: a write the hold skipped resolves `false` and
        // returns before `endInput()`.
        for receiver in held {
            match receiver.await {
                Ok(Ok(())) => {}
                Ok(Err(Error::Aborted(_))) | Err(_) => return Ok(()),
                Ok(Err(error)) => return Err(error.into()),
            }
        }
        if self.prompt_hold.state() == driver::HoldState::Failed {
            return Ok(());
        }
        // `if (count > 0 && hasBidirectionalNeeds()) await waitForRunEnd()`, then `endInput()`: the run-end
        // machine holds stdin open while the run needs it.
        Ok(self.finish_written(Completion::Run, Some(count)).await?)
    }

    /// `claude_agent_sdk` `Query.stream_input`: neither a failing stream nor a failed write reaches the caller.
    async fn python_stream_input<S, E>(&self, mut stream: S)
    where
        S: futures_util::Stream<Item = Result<crate::UserInput, E>> + Unpin,
        E: std::fmt::Display,
    {
        // `written`: the inputs this call wrote.
        let mut written = 0;
        while let Some(item) = stream.next().await {
            if self.close.fired() {
                break;
            }
            let outcome = match item {
                Ok(input) => self.send_input(input).await.map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            if let Err(error) = outcome {
                tracing::error!(%error, "Prompt stream failed; closing stdin");
                break;
            }
            written += 1;
        }
        // `wait_for_result_and_end_input()` when something was written, else `end_input()` at once; the run-end
        // machine makes that choice, and an error closing the input is only logged upstream.
        if let Err(error) = self.finish_written(Completion::Run, Some(written)).await {
            tracing::debug!(%error, "Error closing input stream");
        }
    }
    pub async fn send_prompt(&self, prompt: Prompt) -> Result<(), Error> {
        self.send_input(prompt.into()).await
    }

    /// Writes one user input. The CLI queues it when a turn is already running.
    pub async fn send_input(&self, input: crate::UserInput) -> Result<(), Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::Prompt {
            input: Box::new(input),
            response,
        })
        .await?;
        receiver.await.map_err(|_| self.stopped_error())?
    }

    pub async fn interrupt(&self) -> Result<crate::InterruptReceipt, Error> {
        let response = self
            .control(
                ControlOperation::Interrupt,
                RequestPayload::Interrupt { cancel_queued: None }.to_value()?,
            )
            .await?;
        Ok(interrupt_receipt(response))
    }

    pub async fn interrupt_cancel_queued(&self) -> Result<crate::InterruptReceipt, Error> {
        let response = self
            .control(
                ControlOperation::Interrupt,
                RequestPayload::Interrupt {
                    cancel_queued: Some(true),
                }
                .to_value()?,
            )
            .await?;
        Ok(interrupt_receipt(response))
    }

    /// `None` is TypeScript `setModel()`, which leaves `model` out, or Python `set_model(None)`, which sends
    /// `null`.
    pub async fn set_model(&self, model: Option<String>) -> Result<(), Error> {
        let model = match (model.as_deref(), self.style) {
            (Some(model), _) => Some(Some(model)),
            (None, crate::ArgvStyle::TypeScript) => None,
            (None, crate::ArgvStyle::Python) => Some(None),
        };
        self.control(
            ControlOperation::SetModel,
            RequestPayload::SetModel { model }.to_value()?,
        )
        .await?;
        Ok(())
    }

    /// TypeScript `setMcpPermissionModeOverride`; `None` clears the override. An absent response reads as `{}`.
    pub async fn set_mcp_permission_mode_override(
        &self,
        server_name: &str,
        mode: Option<crate::McpPermissionModeOverride>,
    ) -> Result<ControlReply<JsonObject>, Error> {
        let response = self
            .control(
                ControlOperation::SetMcpPermissionModeOverride,
                RequestPayload::SetMcpPermissionModeOverride {
                    server_name,
                    mode: mode.map(crate::McpPermissionModeOverride::as_str),
                }
                .to_value()?,
            )
            .await?;
        Ok(ControlReply::new(match response {
            None | Some(serde_json::Value::Null) => Some(serde_json::Value::Object(JsonObject::new())),
            response => response,
        }))
    }

    /// TypeScript `setUiHost`: the host that answers the CLI's `ui_*` requests; `None` removes it. A request whose
    /// method is absent is answered as unsupported. `ArgvStyle::Python` never reaches it.
    pub fn set_ui_host(&self, host: Option<crate::UiHost>) {
        self.host.set_ui_host(host);
    }

    /// TypeScript `enableRemoteControl(enabled, name, options)`. A work secret with a refresher installs the
    /// answerer for `remote_control_work_secret`, kept only when the CLI reports a `bridge_session_id` that names
    /// the same session. Needs `ArgvStyle::TypeScript`; Python has no such control request.
    ///
    /// Returns the frame's `response` as `sdk.mjs` does: `None` is `undefined` (the frame had no `response`
    /// key) and `Some(Value::Null)` is `null`. When this is still the latest call, enabling with either one fails
    /// like `sdk.mjs` reading `bridge_session_id` off it, and the answerer installed for the call stays.
    pub async fn enable_remote_control(
        &self,
        enabled: bool,
        name: Option<&str>,
        options: Option<crate::RemoteControlOptions>,
    ) -> Result<Option<serde_json::Value>, Error> {
        let options = options.unwrap_or_default();
        let next = options.answerer(enabled);
        let (generation, previous) = self.host.begin_remote_control(next.clone());
        if enabled {
            self.host
                .arm_remote_control(generation, [next.clone(), previous.clone()]);
        }
        let request = RequestPayload::RemoteControl {
            enabled,
            name,
            reattach_session_id: options.reattach_session_id.as_deref(),
            keep_session_on_exit: options.keep_session_on_exit,
            work_secret: options.work_secret.as_ref().map(crate::SensitiveString::expose),
        }
        .to_value()?;
        let response = match self.control(ControlOperation::RemoteControl, request).await {
            Ok(response) => response,
            Err(error) => {
                self.host.remote_control_failed(generation);
                return Err(error);
            }
        };
        if enabled
            && !self
                .host
                .remote_control_enabled(generation, response.as_ref(), [next, previous])
        {
            return Err(missing_response(ControlOperation::RemoteControl));
        }
        Ok(response)
    }

    /// TypeScript `setMaxThinkingTokens(tokens)`: `thinking_display` is left out.
    pub async fn set_max_thinking_tokens(&self, max_thinking_tokens: Option<u64>) -> Result<(), Error> {
        self.send_max_thinking_tokens(max_thinking_tokens, None).await
    }

    /// TypeScript `setMaxThinkingTokens(tokens, display)`; `None` sends `thinking_display: null`.
    pub async fn set_max_thinking_tokens_with_display(
        &self,
        max_thinking_tokens: Option<u64>,
        thinking_display: Option<crate::MaxThinkingTokensDisplay>,
    ) -> Result<(), Error> {
        self.send_max_thinking_tokens(
            max_thinking_tokens,
            Some(thinking_display.map(crate::MaxThinkingTokensDisplay::as_str)),
        )
        .await
    }

    async fn send_max_thinking_tokens(
        &self,
        max_thinking_tokens: Option<u64>,
        thinking_display: Option<Option<&'static str>>,
    ) -> Result<(), Error> {
        self.control(
            ControlOperation::SetMaxThinkingTokens,
            RequestPayload::SetMaxThinkingTokens {
                max_thinking_tokens,
                thinking_display,
            }
            .to_value()?,
        )
        .await?;
        Ok(())
    }

    /// TypeScript `applyFlagSettings`: the object is sent as given, so a `null` value clears that flag setting.
    pub async fn apply_flag_settings(&self, settings: &JsonObject) -> Result<(), Error> {
        self.control(
            ControlOperation::ApplyFlagSettings,
            RequestPayload::ApplyFlagSettings { settings }.to_value()?,
        )
        .await?;
        Ok(())
    }

    /// TypeScript `updateSettings`.
    pub async fn update_settings(
        &self,
        source: crate::SettingsUpdateSource,
        settings: &JsonObject,
    ) -> Result<(), Error> {
        self.control(
            ControlOperation::UpdateSettings,
            RequestPayload::UpdateSettings {
                source: source.as_str(),
                settings,
            }
            .to_value()?,
        )
        .await?;
        Ok(())
    }

    pub async fn set_permission_mode(&self, mode: crate::PermissionMode) -> Result<(), Error> {
        self.control(
            ControlOperation::SetPermissionMode,
            RequestPayload::SetPermissionMode { mode: mode.as_str() }.to_value()?,
        )
        .await?;
        Ok(())
    }

    /// TypeScript `rewindFiles(id, {dryRun})`, Python `rewind_files(id)`. `None` leaves `dry_run` out, as both do
    /// without the option.
    pub async fn rewind_files(
        &self,
        user_message_id: crate::MessageId,
        mode: impl Into<Option<crate::RewindMode>>,
    ) -> Result<ControlReply<wire_types::RewindFilesResult>, Error> {
        self.reply(
            ControlOperation::RewindFiles,
            RequestPayload::RewindFiles {
                user_message_id: user_message_id.as_str(),
                dry_run: mode.into().map(|mode| mode == crate::RewindMode::DryRun),
            }
            .to_value()?,
        )
        .await
    }

    /// Python `get_mcp_status()`: the whole response.
    pub async fn mcp_status(&self) -> Result<ControlReply<crate::McpStatusResponse>, Error> {
        self.reply(ControlOperation::McpStatus, RequestPayload::McpStatus.to_value()?)
            .await
    }

    /// TypeScript `mcpServerStatus()`: the response's `mcpServers`.
    pub async fn mcp_server_status(&self) -> Result<ControlReply<Vec<wire_types::McpServerStatus>>, Error> {
        let response = self
            .control(ControlOperation::McpStatus, RequestPayload::McpStatus.to_value()?)
            .await?;
        let Some(serde_json::Value::Object(mut response)) = response else {
            return Err(missing_response(ControlOperation::McpStatus));
        };
        Ok(ControlReply::new(response.remove("mcpServers")))
    }

    /// TypeScript `getContextUsage({detail})`, Python `get_context_usage()` when `detail` is `None`.
    pub async fn context_usage(
        &self,
        detail: Option<crate::ContextUsageDetail>,
    ) -> Result<ControlReply<wire_types::SDKControlGetContextUsageResponse>, Error> {
        self.reply(
            ControlOperation::ContextUsage,
            RequestPayload::GetContextUsage {
                detail: detail.map(crate::ContextUsageDetail::as_str),
            }
            .to_value()?,
        )
        .await
    }

    /// TypeScript `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({skipBehaviors})`, which upstream
    /// marks unstable. `skip_behaviors` is sent only when set.
    pub async fn usage_experimental(
        &self,
        skip_behaviors: bool,
    ) -> Result<ControlReply<wire_types::SDKControlGetUsageResponse>, Error> {
        self.reply(
            ControlOperation::GetUsage,
            RequestPayload::GetUsage {
                skip_behaviors: skip_behaviors.then_some(true),
            }
            .to_value()?,
        )
        .await
    }

    /// TypeScript `readFile(path, {maxBytes, encoding})`: `None` for any failure, the CLI's refusal included.
    pub async fn read_file(
        &self,
        path: &str,
        options: crate::ReadFileOptions,
    ) -> Option<ControlReply<wire_types::SDKControlReadFileResponse>> {
        let request = RequestPayload::ReadFile {
            path,
            max_bytes: options.max_bytes,
            encoding: options.encoding.map(crate::ReadFileEncoding::as_str),
        }
        .to_value()
        .ok()?;
        self.reply(ControlOperation::ReadFile, request).await.ok()
    }

    /// TypeScript `reloadPlugins({holdOnCacheImpact, addPlugins})`.
    pub async fn reload_plugins(
        &self,
        options: &crate::ReloadPluginsOptions,
    ) -> Result<ControlReply<wire_types::SDKControlReloadPluginsResponse>, Error> {
        self.reply(
            ControlOperation::ReloadPlugins,
            RequestPayload::ReloadPlugins {
                hold_on_cache_impact: options.hold_on_cache_impact.then_some(true),
                add_plugins: (!options.add_plugins.is_empty()).then(|| {
                    options
                        .add_plugins
                        .iter()
                        .map(crate::plugin::PluginPayload::from)
                        .collect()
                }),
            }
            .to_value()?,
        )
        .await
    }

    /// TypeScript `reloadSkills()`.
    pub async fn reload_skills(&self) -> Result<ControlReply<wire_types::SDKControlReloadSkillsResponse>, Error> {
        self.reply(ControlOperation::ReloadSkills, RequestPayload::ReloadSkills.to_value()?)
            .await
    }

    /// TypeScript `reloadOutputStyles()`.
    pub async fn reload_output_styles(
        &self,
    ) -> Result<ControlReply<wire_types::SDKControlReloadOutputStylesResponse>, Error> {
        self.reply(
            ControlOperation::ReloadOutputStyles,
            RequestPayload::ReloadOutputStyles.to_value()?,
        )
        .await
    }

    /// TypeScript `seedReadState(path, mtime)`. `mtime` is a JavaScript number, sent as `JSON.stringify` writes
    /// it.
    pub async fn seed_read_state(&self, path: &str, mtime: f64) -> Result<(), Error> {
        self.control(
            ControlOperation::SeedReadState,
            RequestPayload::SeedReadState {
                path,
                mtime: javascript_number(mtime),
            }
            .to_value()?,
        )
        .await?;
        Ok(())
    }

    pub async fn reconnect_mcp_server(&self, server_name: crate::McpServerName) -> Result<(), Error> {
        self.control(
            ControlOperation::McpReconnect,
            RequestPayload::McpReconnect {
                server_name: server_name.as_str(),
            }
            .to_value()?,
        )
        .await?;
        Ok(())
    }

    pub async fn toggle_mcp_server(
        &self,
        server_name: crate::McpServerName,
        enabled: crate::McpEnabled,
    ) -> Result<(), Error> {
        self.control(
            ControlOperation::McpToggle,
            RequestPayload::McpToggle {
                server_name: server_name.as_str(),
                enabled: enabled == crate::McpEnabled::Enabled,
            }
            .to_value()?,
        )
        .await?;
        Ok(())
    }

    /// TypeScript `readMcpResource`: reads a resource of a connected MCP server through Claude Code, which
    /// refuses any URI outside the `ui://` scheme. The reply's `response` field is returned as sent, and `None`
    /// stands for the `undefined` TypeScript returns when the reply has no such field.
    pub async fn read_mcp_resource(
        &self,
        server_name: crate::McpServerName,
        uri: &str,
    ) -> Result<Option<crate::McpReadResourceResponse>, Error> {
        let request = RequestPayload::McpReadResource {
            server_name: server_name.as_str(),
            uri,
        }
        .to_value()?;
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::RawControl {
            operation: ControlOperation::McpReadResource,
            request,
            response,
        })
        .await?;
        let response = receiver.await.map_err(|_| self.stopped_error())??;
        Ok(response.map(crate::McpReadResourceResponse::from))
    }

    pub async fn stop_task(&self, task_id: crate::TaskId) -> Result<(), Error> {
        self.control(
            ControlOperation::StopTask,
            RequestPayload::StopTask {
                task_id: task_id.as_str(),
            }
            .to_value()?,
        )
        .await?;
        Ok(())
    }

    /// TypeScript `backgroundTasks(toolUseId)`: `response.backgrounded ?? true`, of whatever type the CLI sent;
    /// [`Backgrounded::into_bool`](crate::Backgrounded::into_bool) is the opt-in boolean check. A reply with no
    /// `response`, or a null one, is a protocol error where TypeScript throws reading `backgrounded` from it, and any
    /// other response that is not an object reads as `true`, like the property of a primitive.
    pub async fn background_tasks(&self, tool_use_id: Option<crate::ToolUseId>) -> Result<crate::Backgrounded, Error> {
        let response = self
            .control(
                ControlOperation::BackgroundTasks,
                RequestPayload::BackgroundTasks {
                    tool_use_id: tool_use_id.as_ref().map(crate::ToolUseId::as_str),
                }
                .to_value()?,
            )
            .await?;
        match response {
            None | Some(serde_json::Value::Null) => Err(missing_response(ControlOperation::BackgroundTasks)),
            Some(serde_json::Value::Object(response)) => {
                Ok(crate::Backgrounded::from_response(response.get("backgrounded")))
            }
            Some(_) => Ok(crate::Backgrounded::Bool(true)),
        }
    }

    async fn reply<T>(
        &self,
        operation: ControlOperation,
        request: serde_json::Value,
    ) -> Result<ControlReply<T>, Error> {
        self.control(operation, request).await.map(ControlReply::new)
    }

    pub async fn finish_input(&self) -> Result<(), Error> {
        self.finish_with(Completion::Run).await
    }

    /// TypeScript `abortController.abort()`: cancels the abort token, which stops the process now and ends the run
    /// with `Error::Aborted`. Later writes fail with `AbortError("Operation aborted")`.
    pub async fn abort(&self) -> Result<(), Error> {
        self.abort.cancel();
        Ok(())
    }

    /// TypeScript `Query.close()`, from any handle: shuts the process down without waiting. Idempotent.
    pub fn close(&self) {
        self.close.fire();
    }

    /// Queues a prompt without waiting for the driver to write it.
    async fn queue_input(&self, input: crate::UserInput) -> Result<(), Error> {
        let (response, _receiver) = oneshot::channel();
        self.enqueue(Command::Prompt {
            input: Box::new(input),
            response,
        })
        .await
    }

    /// Queues the end of the input without waiting for the driver to apply it.
    async fn queue_finish(&self, completion: Completion) -> Result<(), Error> {
        let (response, _receiver) = oneshot::channel();
        let command = Command::FinishInput {
            completion,
            written: None,
            response,
        };
        match self.send_command(command).await {
            Err(_) => Err(Error::CommandQueueTimeout),
            // A stopped driver makes ending the input a no-op, as in `finish_with`.
            Ok(_) => Ok(()),
        }
    }

    pub(crate) async fn finish_with(&self, completion: Completion) -> Result<(), Error> {
        self.finish_written(completion, None).await
    }

    /// [`finish_with`](Self::finish_with) for a caller that counted its own prompts, as `streamInput` and
    /// `stream_input` do per call.
    async fn finish_written(&self, completion: Completion, written: Option<usize>) -> Result<(), Error> {
        let (response, receiver) = oneshot::channel();
        let command = Command::FinishInput {
            completion,
            written,
            response,
        };
        match self.send_command(command).await {
            Err(_) => Err(Error::CommandQueueTimeout),
            // Upstream `endInput()` (TypeScript) and `end_input()` (Python) only close stdin and never fail, so ending
            // the input of a run that already stopped is a no-op; the run's own error surfaces from the message
            // stream.
            Ok(Err(_)) => Ok(()),
            Ok(Ok(())) => receiver.await.unwrap_or(Ok(())),
        }
    }

    /// Queues a control request without waiting for its answer, so a caller can queue several in order.
    pub(crate) async fn control_started(
        &self,
        operation: ControlOperation,
        request: serde_json::Value,
    ) -> Result<ControlTicket, Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::Control {
            operation,
            request,
            response,
        })
        .await?;
        Ok(ControlTicket {
            handle: self.clone(),
            receiver,
        })
    }

    /// Resolves once the process has stopped, however it ended.
    pub(crate) async fn stopped(&self) {
        self.commands.closed().await;
    }

    pub(crate) fn set_claim_pending(&self, pending: bool) {
        self.claim_pending.send_replace(pending);
    }

    /// TypeScript reads such as `initializationResult()` of a claimed spare wait for the claim's answer, then
    /// describe the claimed session. Resolves at once when no claim is waiting for its answer.
    async fn wait_for_claim(&self) {
        let mut pending = self.claim_pending.subscribe();
        let _ = pending.wait_for(|pending| !*pending).await;
    }

    async fn control(
        &self,
        operation: ControlOperation,
        request: serde_json::Value,
    ) -> Result<Option<serde_json::Value>, Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::Control {
            operation,
            request,
            response,
        })
        .await?;
        receiver.await.map_err(|_| self.stopped_error())?
    }

    async fn enqueue(&self, command: Command) -> Result<(), Error> {
        // TypeScript `write()` checks the abort signal before anything else.
        if self.abort.is_cancelled() {
            return Err(crate::AbortError::OPERATION.into());
        }
        self.send_command(command)
            .await
            .map_err(|_| Error::CommandQueueTimeout)?
            .map_err(|_| self.stopped_error())
    }

    async fn send_command(&self, command: Command) -> Result<Result<(), Command>, tokio::time::error::Elapsed> {
        match self.timeout {
            Some(limit) => timeout(limit, self.commands.send(command)).await,
            None => Ok(self.commands.send(command).await),
        }
    }

    /// The error for a write that the stopped driver did not (or could not) accept. The driver records how the
    /// process ended before it drops its queue, so a command still buffered when it stopped gets the same error as one
    /// sent afterwards.
    fn stopped_error(&self) -> Error {
        // TypeScript: a write after the abort signal fired throws `AbortError("Operation aborted")`.
        if self.abort.is_cancelled() {
            crate::AbortError::OPERATION.into()
        } else {
            self.closed.write_error().unwrap_or(Error::DriverStopped)
        }
    }
}

/// The queue report of an interrupt response; anything but an object reports nothing.
fn interrupt_receipt(response: Option<serde_json::Value>) -> crate::InterruptReceipt {
    match response {
        Some(serde_json::Value::Object(object)) => crate::InterruptReceipt::from_json(object),
        _ => crate::InterruptReceipt::from_json(JsonObject::new()),
    }
}

/// TypeScript `claimSession`'s merge. It reads the applied flags off the earlier result, none when that failed,
/// which throws when the result is missing or `null`; then off the claim's `initialize`, which throws when that
/// is `null`. It spreads the `initialize` into a new object and sets each flag, the claim's unless that is
/// missing or `null`, that is not `undefined`.
fn claim_initialization(
    previous: Option<&InitializationOutcome>,
    update: &serde_json::Value,
    runtime: crate::JsRuntime,
) -> InitializationOutcome {
    let earlier = match previous {
        Some(Ok(value)) => {
            if let Some(nothing) = Nullish::of(value.as_ref()) {
                return Err(std::sync::Arc::new(runtime.read_error(
                    nothing,
                    "hooks_applied",
                    "d.hooks_applied",
                )));
            }
            value.as_ref().and_then(serde_json::Value::as_object)
        }
        // A failed result keeps no flags. A spare is handed out only once its initialize settled, so an unsettled
        // one never reaches here.
        Some(Err(_)) | None => None,
    };
    if update.is_null() {
        return Err(std::sync::Arc::new(runtime.read_error(
            Nullish::Null,
            "hooks_applied",
            "o.hooks_applied",
        )));
    }
    let mut merged = spread(update);
    let fresh = update.as_object();
    for key in CLAIM_FLAGS {
        let value = fresh
            .and_then(|fresh| fresh.get(key))
            .filter(|value| !value.is_null())
            .or_else(|| earlier.and_then(|earlier| earlier.get(key)));
        if let Some(value) = value {
            merged.insert(key.to_owned(), value.clone());
        }
    }
    Ok(Some(serde_json::Value::Object(merged)))
}

/// JavaScript `{...value}` for a value other than `null`: an object copies its entries, an array its items and a
/// string its UTF-16 code units under their indexes, and a number or boolean spreads nothing. Half of a surrogate
/// pair cannot be held in UTF-8, so it reads as U+FFFD.
fn spread(value: &serde_json::Value) -> JsonObject {
    match value {
        serde_json::Value::Object(object) => object.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item.clone()))
            .collect(),
        serde_json::Value::String(text) => text
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| {
                let character = char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER);
                (index.to_string(), serde_json::Value::String(character.to_string()))
            })
            .collect(),
        _ => JsonObject::new(),
    }
}

/// TypeScript reads a field of the response, which throws when the CLI sent none.
fn missing_response(operation: ControlOperation) -> Error {
    ProtocolError::from(<serde_json::Error as serde::de::Error>::custom(format!(
        "{operation} answered without a response object"
    )))
    .into()
}

/// A JavaScript number: an integral value as an integer, NaN and the infinities as `null`.
fn javascript_number(value: f64) -> serde_json::Value {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if value.fract() == 0.0 && value.abs() <= EXACT {
        #[allow(clippy::cast_possible_truncation, reason = "the value is an integer within 2^53")]
        let integer = value as i64;
        return serde_json::Value::from(integer);
    }
    serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

impl ClaudeSdkClient {
    /// A query over this connection whose prompt the caller writes, as `SpareProcess::claim` does.
    pub(crate) fn into_query(self) -> Query {
        Query {
            client: Some(self),
            one_turn: false,
            result_received: false,
            sse: None,
        }
    }
}

/// Messages through run completion, including background-agent continuation turns.
pub struct Query {
    client: Option<ClaudeSdkClient>,
    one_turn: bool,
    result_received: bool,
    /// The SSE transport's operations, for a query opened with [`crate::browser_query`] over SSE.
    sse: Option<crate::SseHandle>,
}

pub async fn query(prompt: impl Into<crate::UserInput>, options: ClaudeAgentOptions) -> Result<Query, Error> {
    query_with_completion(prompt, options, Completion::Run).await
}

pub async fn query_turn(prompt: impl Into<crate::UserInput>, options: ClaudeAgentOptions) -> Result<Query, Error> {
    query_with_completion(prompt, options, Completion::Turn).await
}

async fn query_with_completion(
    prompt: impl Into<crate::UserInput>,
    options: ClaudeAgentOptions,
    completion: Completion,
) -> Result<Query, Error> {
    let adapters: crate::adapters::RuntimeAdapters = crate::adapters::RuntimeAdapters::default();
    query_with_completion_and_adapters(prompt, options, completion, adapters).await
}

pub async fn query_with_adapters<E, P, F>(
    prompt: impl Into<crate::UserInput>,
    options: ClaudeAgentOptions,
    adapters: crate::adapters::RuntimeAdapters<E, P, F>,
) -> Result<Query, Error>
where
    E: crate::adapters::EnvironmentVariable
        + crate::adapters::CurrentDirectory<Error = std::io::Error>
        + crate::adapters::TemporaryDirectory,
    P: crate::adapters::SpawnProcess<Error = std::io::Error>,
    F: crate::adapters::SessionFileSystem,
{
    query_with_completion_and_adapters(prompt, options, Completion::Run, adapters).await
}

pub async fn query_turn_with_adapters<E, P, F>(
    prompt: impl Into<crate::UserInput>,
    options: ClaudeAgentOptions,
    adapters: crate::adapters::RuntimeAdapters<E, P, F>,
) -> Result<Query, Error>
where
    E: crate::adapters::EnvironmentVariable
        + crate::adapters::CurrentDirectory<Error = std::io::Error>
        + crate::adapters::TemporaryDirectory,
    P: crate::adapters::SpawnProcess<Error = std::io::Error>,
    F: crate::adapters::SessionFileSystem,
{
    query_with_completion_and_adapters(prompt, options, Completion::Turn, adapters).await
}

async fn query_with_completion_and_adapters<E, P, F>(
    prompt: impl Into<crate::UserInput>,
    options: ClaudeAgentOptions,
    completion: Completion,
    adapters: crate::adapters::RuntimeAdapters<E, P, F>,
) -> Result<Query, Error>
where
    E: crate::adapters::EnvironmentVariable
        + crate::adapters::CurrentDirectory<Error = std::io::Error>
        + crate::adapters::TemporaryDirectory,
    P: crate::adapters::SpawnProcess<Error = std::io::Error>,
    F: crate::adapters::SessionFileSystem,
{
    let client = Box::pin(ClaudeSdkClient::connect_resolved(
        options,
        adapters,
        None,
        false,
        None,
        PythonCStack::QUERY,
    ))
    .await?;
    Box::pin(start_query(client, prompt.into(), completion, PromptShape::SdkMjs)).await
}

/// Python `query(prompt, transport=...)`.
pub async fn query_with_transport<T: crate::Transport>(
    prompt: impl Into<crate::UserInput>,
    options: ClaudeAgentOptions,
    transport: T,
) -> Result<Query, Error> {
    let client = Box::pin(ClaudeSdkClient::connect_with_transport(options, transport)).await?;
    Box::pin(start_query(client, prompt.into(), Completion::Run, PromptShape::Python)).await
}

/// [`query_with_transport`] that ends after the first result.
pub async fn query_turn_with_transport<T: crate::Transport>(
    prompt: impl Into<crate::UserInput>,
    options: ClaudeAgentOptions,
    transport: T,
) -> Result<Query, Error> {
    let client = Box::pin(ClaudeSdkClient::connect_with_transport(options, transport)).await?;
    Box::pin(start_query(
        client,
        prompt.into(),
        Completion::Turn,
        PromptShape::Python,
    ))
    .await
}

/// Whose `query()` a one-shot prompt comes from, which decides how a text prompt is written under the TypeScript
/// style: `sdk.mjs` wraps it in a text block, Python writes it as a string.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PromptShape {
    /// `query()` and `WarmQuery.query()` in `sdk.mjs`, which wrap a string prompt in a single text block.
    SdkMjs,
    /// Python `query(prompt, transport=...)`, which writes a string prompt as is.
    Python,
}

pub(crate) async fn start_query(
    client: ClaudeSdkClient,
    prompt: crate::UserInput,
    completion: Completion,
    shape: PromptShape,
) -> Result<Query, Error> {
    // `sdk.mjs` treats a prompt as a single user turn: stdin closes at the result, or at session idle when
    // something needs the CLI to keep asking. Python closes it once the prompt was written, which the run rule
    // does. The reader keeps going until the CLI exits either way; only `query_turn` stops at the result.
    let settle = match (completion, client.runtime.handle.style) {
        (Completion::Run, crate::ArgvStyle::TypeScript) => Completion::Turn,
        (completion, _) => completion,
    };
    let startup = match client.runtime.handle.style {
        // `sdk.mjs` writes the prompt and ends the input without waiting for the initialize response, which a
        // blocking hook or managed settings can delay. Both are queued, so they follow the initialize request.
        crate::ArgvStyle::TypeScript => {
            let prompt = prompt.into_query_prompt(matches!(shape, PromptShape::SdkMjs));
            async {
                client.runtime.handle.queue_input(prompt).await?;
                client.runtime.handle.queue_finish(settle).await
            }
            .await
        }
        // Python `query(prompt=str)` writes the text as a string, with `session_id` ahead of `message`.
        crate::ArgvStyle::Python => {
            async {
                client.send_input(prompt.into_query_prompt(false)).await?;
                client.runtime.handle.finish_with(settle).await
            }
            .await
        }
    };
    if let Err(error) = startup {
        return match client.disconnect().await {
            Ok(()) => Err(error),
            Err(cleanup) => Err(Error::Cleanup {
                primary: Box::new(error),
                cleanup: Box::new(cleanup),
            }),
        };
    }
    Ok(Query {
        client: Some(client),
        one_turn: matches!(completion, Completion::Turn),
        result_received: false,
        sse: None,
    })
}

impl Query {
    /// A query over a connected client whose input is streamed separately, ending with the run.
    pub(crate) fn streaming(client: ClaudeSdkClient, sse: Option<crate::SseHandle>) -> Self {
        Self {
            client: Some(client),
            one_turn: false,
            result_received: false,
            sse,
        }
    }

    pub(crate) fn sse_handle(&self) -> Option<&crate::SseHandle> {
        self.sse.as_ref()
    }

    pub fn handle(&self) -> Option<ClientHandle> {
        self.client.as_ref().map(ClaudeSdkClient::handle)
    }

    /// Cancellation-safe: cancelling this wait does not discard a message.
    pub async fn next_message(&mut self) -> Result<Option<Message>, Error> {
        std::future::poll_fn(|cx| self.poll_next_message(cx)).await
    }

    /// TypeScript `Query.close()`: shuts the process down without waiting, and may be called while another task
    /// reads. Idempotent. Messages already received stay readable, after which the query ends; control requests
    /// still waiting fail with `Error::QueryClosed`. [`Query::disconnect`] closes and also waits for cleanup.
    pub fn close(&self) {
        if let Some(client) = &self.client {
            client.close();
        }
    }

    /// Closes the process and waits for shutdown and final storage work, reporting cleanup errors. What the
    /// TypeScript iterator's `return()` awaits; messages not read yet are discarded.
    pub async fn disconnect(mut self) -> Result<(), Error> {
        match self.client.take() {
            Some(client) => client.disconnect().await,
            None => Ok(()),
        }
    }
}

#[path = "client_stream.rs"]
mod stream;

#[path = "client_connect.rs"]
mod connect;
pub use connect::{ClientConnector, ConnectPrompt, CustomTransport, NativeCli, UnconnectedClient};

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
