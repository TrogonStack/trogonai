use futures_util::StreamExt;

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};

use crate::{
    ClaudeAgentOptions, ControlOperation, Error, JsonObject, Message, Prompt, ProtocolError,
    driver::{self, Command, Completion},
    process_transport::{Closed, ProcessSpawn, ProcessTransport},
    wire::runtime_payload::RequestPayload,
};

struct StartInput<T, F: crate::adapters::SessionFileSystem> {
    options: ClaudeAgentOptions,
    session: Option<crate::SessionRuntime<F>>,
    transport: T,
    initialization: driver::Initialization,
    version_check: Option<String>,
    closed: Closed,
    process_id: Option<u32>,
}

/// Owns one Claude Code subprocess. Keep consuming messages while a turn runs.
pub struct ClaudeSdkClient {
    runtime: ClientRuntime,
    connection_info: crate::ConnectionInfo,
}

struct ClientRuntime {
    handle: ClientHandle,
    messages: mpsc::Receiver<Message>,
    terminal: Option<oneshot::Receiver<Result<(), Error>>>,
    shutdown: Option<oneshot::Sender<()>>,
    driver: Option<JoinHandle<()>>,
    process_id: Option<u32>,
}

/// A cloneable command handle that does not extend the subprocess owner's life.
#[derive(Clone)]
pub struct ClientHandle {
    commands: mpsc::Sender<Command>,
    timeout: std::time::Duration,
    aborted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    closed: Closed,
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
    inherited.extend(environment.vars_os().into_iter().filter(|(key, _)| {
        key.to_string_lossy()
            .eq_ignore_ascii_case("CLAUDE_CODE_SDK_READS_SESSION_STATE")
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
                &inherited,
                &adapters.filesystem,
                cfg!(windows),
            )?)
        } else {
            None
        };
        Self::connect_resolved(options, adapters, discovered).await
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
        Self::connect_resolved(options, adapters, None).await
    }

    async fn connect_resolved<E, P, F>(
        mut options: ClaudeAgentOptions,
        adapters: crate::adapters::RuntimeAdapters<E, P, F>,
        discovered: Option<std::path::PathBuf>,
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
        let session = Self::prepare_session(
            &mut options,
            &adapters.environment,
            &adapters.process,
            adapters.filesystem,
            true,
        )
        .await?;
        let initialization = driver::Initialization::new(&options)?;
        let closed = Closed::default();
        let transport = ProcessTransport::spawn(ProcessSpawn {
            options: &options,
            cli_path: cli_path.clone(),
            inherited: &inherited,
            process: &adapters.process,
            config_dir: session.as_ref().and_then(|session| session.config_dir.as_deref()),
            closed: closed.clone(),
        })?;
        let version_check = (!crate::version_check::skip_requested(
            inherited
                .get(std::ffi::OsStr::new("CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK"))
                .map(|v| v.as_os_str()),
        ))
        .then(|| cli_path.display().to_string());
        let process_id = transport.process_id();
        Self::start(StartInput {
            options,
            session,
            transport,
            initialization,
            version_check,
            closed,
            process_id,
        })
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
        Self::connect_with_transport_and_adapters(options, transport, adapters).await
    }

    /// [`ClaudeSdkClient::connect_with_transport`] with caller-supplied adapters for the session store mirror.
    pub async fn connect_with_transport_and_adapters<T, E, P, F>(
        mut options: ClaudeAgentOptions,
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
        let session = Self::prepare_session(
            &mut options,
            &adapters.environment,
            &adapters.process,
            adapters.filesystem,
            false,
        )
        .await?;
        let initialization = driver::Initialization::new(&options)?;
        Self::start(StartInput {
            options,
            session,
            transport,
            initialization,
            version_check: None,
            closed: Closed::default(),
            process_id: None,
        })
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
        let cwd = match options.cwd.clone() {
            Some(cwd) => cwd,
            None => environment.current_dir().map_err(Error::Io)?,
        };
        // The CLI must run in the directory the session project key was derived from.
        options.cwd = Some(cwd.clone());
        let runtime = crate::SessionRuntime::prepare_with_adapters(crate::SessionPreparationOptions {
            store_options: &store_options,
            resume,
            cwd: &cwd,
            subprocess_environment: &options.env,
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
        } = input;
        let (command_tx, command_rx) = mpsc::channel(options.limits.command_queue_capacity.get());
        let (message_tx, message_rx) = mpsc::channel(options.limits.message_queue_capacity.get());
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = ClientHandle {
            commands: command_tx,
            timeout: options.limits.control_timeout.as_duration(),
            aborted: std::sync::Arc::default(),
            closed,
        };
        let driver = driver::spawn(driver::SpawnOptions {
            options,
            session,
            commands: command_rx,
            messages: message_tx,
            shutdown: shutdown_rx,
            terminal: terminal_tx,
            transport,
            initialization,
            version_check,
        })
        .await?;
        let mut runtime = ClientRuntime {
            handle,
            messages: message_rx,
            terminal: Some(terminal_rx),
            shutdown: Some(shutdown_tx),
            driver: Some(driver),
            process_id,
        };
        match runtime.handle.initialize().await {
            Ok(connection_info) => Ok(Self {
                runtime,
                connection_info,
            }),
            Err(error) => {
                let cleanup = if matches!(error, Error::DriverStopped) {
                    runtime.finish().await
                } else {
                    runtime.close().await
                };
                Err(connection_failure(error, cleanup))
            }
        }
    }

    pub fn handle(&self) -> ClientHandle {
        self.runtime.handle.clone()
    }

    pub fn process_id(&self) -> Option<u32> {
        self.runtime.process_id
    }

    /// Metadata reported during connection, without another protocol exchange.
    pub fn connection_info(&self) -> &crate::ConnectionInfo {
        &self.connection_info
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

    pub async fn stream_input<S>(&self, mut stream: S) -> Result<(), Error>
    where
        S: futures_util::Stream<Item = crate::UserInput> + Unpin,
    {
        self.runtime.handle.stream_input(&mut stream).await
    }

    pub async fn query(&self, prompt: Prompt) -> Result<(), Error> {
        self.runtime.handle.send_prompt(prompt).await
    }

    /// Cancellation-safe: cancelling this wait does not discard a message.
    pub async fn next_message(&mut self) -> Result<Option<Message>, Error> {
        match self.runtime.messages.recv().await {
            Some(message) => Ok(Some(message)),
            None => {
                self.finish().await?;
                Ok(None)
            }
        }
    }

    /// Waits for process shutdown and final storage work, reporting cleanup errors.
    pub async fn disconnect(self) -> Result<(), Error> {
        self.runtime.close().await
    }

    async fn finish(&mut self) -> Result<(), Error> {
        self.runtime.finish().await
    }
}

fn connection_failure(primary: Error, cleanup: Result<(), Error>) -> Error {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) if matches!(primary, Error::DriverStopped) => cleanup,
        Err(Error::DriverStopped) => primary,
        Err(Error::BlockingHooksUnsupported) if matches!(primary, Error::BlockingHooksUnsupported) => primary,
        Err(Error::HooksNotApplied) if matches!(primary, Error::HooksNotApplied) => primary,
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
        self.request_shutdown();
        self.finish().await
    }

    fn request_shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }

    async fn finish(&mut self) -> Result<(), Error> {
        let outcome = if let Some(terminal) = self.terminal.as_mut() {
            let outcome = terminal.await.map_err(|_| Error::DriverStopped)?;
            self.terminal = None;
            outcome
        } else {
            Ok(())
        };
        if let Some(driver) = self.driver.take() {
            driver.await.map_err(Error::DriverTask)?;
        }
        outcome
    }
}

impl Drop for ClientRuntime {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

impl ClientHandle {
    async fn initialize(&self) -> Result<crate::ConnectionInfo, Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::Initialize { response }).await?;
        receiver.await.map_err(|_| Error::DriverStopped)?
    }

    pub async fn set_mcp_servers(
        &self,
        external: std::collections::BTreeMap<crate::McpServerName, crate::McpServerOptions>,
        embedded: std::collections::BTreeMap<crate::McpServerName, std::sync::Arc<dyn crate::SdkMcpServer>>,
    ) -> Result<crate::McpSetServersResult, Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::SetMcpServers {
            external,
            embedded,
            response,
        })
        .await?;
        let response = receiver.await.map_err(|_| Error::DriverStopped)??;
        serde_json::from_value(response.into())
            .map_err(ProtocolError::from)
            .map_err(Error::from)
    }
    pub async fn stream_input<S>(&self, mut stream: S) -> Result<(), Error>
    where
        S: futures_util::Stream<Item = crate::UserInput> + Unpin,
    {
        while let Some(input) = stream.next().await {
            self.send_input(input).await?;
        }
        self.finish_input().await
    }
    pub async fn send_prompt(&self, prompt: Prompt) -> Result<(), Error> {
        self.send_input(prompt.into()).await
    }

    /// Writes one user input. The CLI queues it when a turn is already running.
    pub async fn send_input(&self, input: crate::UserInput) -> Result<(), Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::Prompt { input, response }).await?;
        receiver.await.map_err(|_| self.stopped_error())?
    }

    pub async fn interrupt(&self) -> Result<crate::InterruptReceipt, Error> {
        let response = self
            .control(
                ControlOperation::Interrupt,
                RequestPayload::Interrupt { cancel_queued: None }.to_value()?,
            )
            .await?;
        Ok(crate::InterruptReceipt::from_json(response))
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
        Ok(crate::InterruptReceipt::from_json(response))
    }

    pub async fn set_model(&self, model: Option<String>) -> Result<(), Error> {
        self.control(
            ControlOperation::SetModel,
            RequestPayload::SetModel {
                model: model.as_deref(),
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

    pub async fn rewind_files(
        &self,
        user_message_id: crate::MessageId,
        mode: crate::RewindMode,
    ) -> Result<crate::RewindFilesResult, Error> {
        self.typed_control(
            ControlOperation::RewindFiles,
            RequestPayload::RewindFiles {
                user_message_id: user_message_id.as_str(),
                dry_run: mode == crate::RewindMode::DryRun,
            }
            .to_value()?,
        )
        .await
    }

    pub async fn mcp_status(&self) -> Result<crate::McpStatusResponse, Error> {
        self.typed_control(ControlOperation::McpStatus, RequestPayload::McpStatus.to_value()?)
            .await
    }

    pub async fn context_usage(&self) -> Result<crate::ContextUsageResponse, Error> {
        self.typed_control(
            ControlOperation::ContextUsage,
            RequestPayload::GetContextUsage.to_value()?,
        )
        .await
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

    pub async fn background_tasks(&self, tool_use_id: Option<crate::ToolUseId>) -> Result<bool, Error> {
        #[derive(serde::Deserialize)]
        struct Response {
            backgrounded: bool,
        }
        let response: Response = self
            .typed_control(
                ControlOperation::BackgroundTasks,
                RequestPayload::BackgroundTasks {
                    tool_use_id: tool_use_id.as_ref().map(crate::ToolUseId::as_str),
                }
                .to_value()?,
            )
            .await?;
        Ok(response.backgrounded)
    }

    async fn typed_control<T: serde::de::DeserializeOwned>(
        &self,
        operation: ControlOperation,
        request: serde_json::Value,
    ) -> Result<T, Error> {
        let response = self.control(operation, request).await?;
        serde_json::from_value(response.into())
            .map_err(ProtocolError::from)
            .map_err(Error::from)
    }

    pub async fn finish_input(&self) -> Result<(), Error> {
        self.finish_with(Completion::Run).await
    }

    pub async fn abort(&self) -> Result<(), Error> {
        self.aborted.store(true, std::sync::atomic::Ordering::SeqCst);
        self.enqueue(Command::Abort).await
    }

    async fn finish_with(&self, completion: Completion) -> Result<(), Error> {
        let (response, receiver) = oneshot::channel();
        let command = Command::FinishInput { completion, response };
        match timeout(self.timeout, self.commands.send(command)).await {
            Err(_) => Err(Error::CommandQueueTimeout),
            // Upstream `endInput()` (TypeScript) and `end_input()` (Python) only close stdin and never fail, so ending
            // the input of a run that already stopped is a no-op; the run's own error surfaces from the message
            // stream.
            Ok(Err(_)) => Ok(()),
            Ok(Ok(())) => receiver.await.unwrap_or(Ok(())),
        }
    }

    async fn control(&self, operation: ControlOperation, request: serde_json::Value) -> Result<JsonObject, Error> {
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
        timeout(self.timeout, self.commands.send(command))
            .await
            .map_err(|_| Error::CommandQueueTimeout)?
            .map_err(|_| self.stopped_error())
    }

    /// The error for a write that the stopped driver did not (or could not) accept. The driver records how the
    /// process ended before it drops its queue, so a command still buffered when it stopped gets the same error as one
    /// sent afterwards.
    fn stopped_error(&self) -> Error {
        // TypeScript: a write after the abort signal fired throws `AbortError("Operation aborted")`.
        if self.aborted.load(std::sync::atomic::Ordering::SeqCst) {
            crate::AbortError::OPERATION.into()
        } else {
            self.closed.write_error().unwrap_or(Error::DriverStopped)
        }
    }
}

/// Messages through run completion, including background-agent continuation turns.
pub struct Query {
    client: Option<ClaudeSdkClient>,
    one_turn: bool,
    result_received: bool,
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
    let client = ClaudeSdkClient::connect_with_adapters(options, adapters).await?;
    start_query(client, prompt.into(), completion).await
}

/// Python `query(prompt, transport=...)`.
pub async fn query_with_transport<T: crate::Transport>(
    prompt: impl Into<crate::UserInput>,
    options: ClaudeAgentOptions,
    transport: T,
) -> Result<Query, Error> {
    let client = ClaudeSdkClient::connect_with_transport(options, transport).await?;
    start_query(client, prompt.into(), Completion::Run).await
}

/// [`query_with_transport`] that ends after the first result.
pub async fn query_turn_with_transport<T: crate::Transport>(
    prompt: impl Into<crate::UserInput>,
    options: ClaudeAgentOptions,
    transport: T,
) -> Result<Query, Error> {
    let client = ClaudeSdkClient::connect_with_transport(options, transport).await?;
    start_query(client, prompt.into(), Completion::Turn).await
}

async fn start_query(
    client: ClaudeSdkClient,
    prompt: crate::UserInput,
    completion: Completion,
) -> Result<Query, Error> {
    let startup = async {
        client.send_input(prompt).await?;
        client.runtime.handle.finish_with(completion).await
    }
    .await;
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
    })
}

impl Query {
    pub fn handle(&self) -> Option<ClientHandle> {
        self.client.as_ref().map(ClaudeSdkClient::handle)
    }

    pub async fn next_message(&mut self) -> Result<Option<Message>, Error> {
        let Some(client) = self.client.as_mut() else {
            return Ok(None);
        };
        if self.one_turn && self.result_received {
            self.disconnect_client().await?;
            return Ok(None);
        }
        let message = client.next_message().await?;
        if matches!(message, Some(Message::Result(_))) {
            self.result_received = true;
        }
        Ok(message)
    }

    pub async fn close(mut self) -> Result<(), Error> {
        self.disconnect_client().await
    }

    async fn disconnect_client(&mut self) -> Result<(), Error> {
        if let Some(client) = self.client.take() {
            client.disconnect().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
