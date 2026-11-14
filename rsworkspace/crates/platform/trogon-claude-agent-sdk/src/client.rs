use futures_util::StreamExt;

use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

use crate::{
    ClaudeAgentOptions, ControlOperation, ControlReply, Error, JsonObject, Message, Prompt, ProtocolError,
    driver::{self, Command, Completion},
    process_transport::{Closed, ProcessSpawn, ProcessTransport},
    upstream::control as wire_types,
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
    messages: crate::channel::Receiver<Message>,
    terminal: Option<oneshot::Receiver<Result<(), Error>>>,
    shutdown: Option<oneshot::Sender<()>>,
    driver: Option<JoinHandle<()>>,
    process_id: Option<u32>,
}

/// A cloneable command handle that does not extend the subprocess owner's life.
#[derive(Clone)]
pub struct ClientHandle {
    commands: crate::channel::Sender<Command>,
    /// Only a bounded command queue can be full, so the wait applies only when the caller opted into both.
    timeout: Option<std::time::Duration>,
    aborted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    closed: Closed,
    /// The first initialize response, which `reinitialize` does not replace; `None` when the reply had none.
    initialization: std::sync::Arc<std::sync::OnceLock<Option<serde_json::Value>>>,
    latest_commands: driver::LatestCommands,
    style: crate::ArgvStyle,
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
                &inherited,
                &adapters.filesystem,
                cfg!(windows),
                options.bundled_cli_root.as_deref(),
            )?)
        } else {
            None
        };
        Self::connect_resolved(options, adapters, discovered, true).await
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
        Self::connect_resolved(options, adapters, None, false).await
    }

    async fn connect_resolved<E, P, F>(
        mut options: ClaudeAgentOptions,
        adapters: crate::adapters::RuntimeAdapters<E, P, F>,
        discovered: Option<std::path::PathBuf>,
        derive_debug_file: bool,
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
        crate::shadow_warning::emit(&options);
        let initialization = driver::Initialization::new(&options, &adapters.environment)?;
        let closed = Closed::default();
        // TypeScript derives a default debug file only when the SDK spawns the CLI and none was given.
        let derived_debug_file = (derive_debug_file
            && options.argv_style == crate::ArgvStyle::TypeScript
            && options
                .debug_file
                .as_ref()
                .is_none_or(|file| file.as_os_str().is_empty()))
        .then(|| debug_probe(&adapters.environment, &inherited))
        .and_then(|probe| crate::debug_file::process_debug_file(&probe));
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
        crate::shadow_warning::emit(&options);
        let initialization = driver::Initialization::new(&options, &adapters.environment)?;
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
        } = input;
        let (command_tx, command_rx) = crate::channel::channel(options.limits.command_queue_capacity);
        let (message_tx, message_rx) = crate::channel::channel(options.limits.message_buffer.capacity());
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = ClientHandle {
            commands: command_tx,
            timeout: options
                .limits
                .command_queue_capacity
                .and(options.limits.command_queue_timeout)
                .map(crate::Timeout::as_duration),
            aborted: std::sync::Arc::default(),
            closed,
            initialization: std::sync::Arc::default(),
            latest_commands: driver::LatestCommands::default(),
            style: options.argv_style,
        };
        let latest_commands = handle.latest_commands.clone();
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
            latest_commands,
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
        match runtime.handle.initialize(true).await {
            Ok(response) => {
                let connection_info = crate::ConnectionInfo::from_response(response.clone());
                let _ = runtime.handle.initialization.set(response);
                Ok(Self {
                    runtime,
                    connection_info,
                })
            }
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
    async fn initialize(&self, first: bool) -> Result<Option<serde_json::Value>, Error> {
        let (response, receiver) = oneshot::channel();
        self.enqueue(Command::Initialize { first, response }).await?;
        receiver.await.map_err(|_| Error::DriverStopped)?
    }

    /// TypeScript `initializationResult()`, Python `get_server_info()`: the first initialize response.
    pub fn initialization_result(&self) -> Result<crate::ConnectionInfo, Error> {
        self.initialization_value().map(crate::ConnectionInfo::from_response)
    }

    fn initialization_value(&self) -> Result<Option<serde_json::Value>, Error> {
        self.initialization.get().cloned().ok_or(Error::NotInitialized)
    }

    /// A field of the first initialize response, `None` when it is missing (TypeScript `undefined`). TypeScript
    /// reads it off the response, which throws when the CLI sent none or `null`.
    fn initialization_field<T>(&self, key: &str) -> Result<ControlReply<T>, Error> {
        match self.initialization_value()? {
            None | Some(serde_json::Value::Null) => Err(missing_response(ControlOperation::Initialize)),
            Some(value) => Ok(ControlReply::new(value.get(key).cloned())),
        }
    }

    /// TypeScript `reinitialize()`: sends the connect-time initialize request again, without SDK MCP server
    /// manifests, and returns its response. A response that fails the initialize checks ends the session.
    /// [`ClientHandle::initialization_result`] keeps the first response.
    pub async fn reinitialize(&self) -> Result<crate::ConnectionInfo, Error> {
        self.initialize(false).await.map(crate::ConnectionInfo::from_response)
    }

    /// TypeScript `supportedCommands()`: the list from the last `commands_changed` message, else the initialize
    /// one.
    pub fn supported_commands(&self) -> Result<ControlReply<Vec<wire_types::SlashCommand>>, Error> {
        let latest = self
            .latest_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match latest {
            Some(commands) => Ok(ControlReply::new(Some(commands))),
            None => self.initialization_field("commands"),
        }
    }

    /// TypeScript `supportedModels()`.
    pub fn supported_models(&self) -> Result<ControlReply<Vec<wire_types::ModelInfo>>, Error> {
        self.initialization_field("models")
    }

    /// TypeScript `supportedAgents()`.
    pub fn supported_agents(&self) -> Result<ControlReply<Vec<wire_types::AgentInfo>>, Error> {
        self.initialization_field("agents")
    }

    /// TypeScript `accountInfo()`.
    pub fn account_info(&self) -> Result<ControlReply<wire_types::AccountInfo>, Error> {
        self.initialization_field("account")
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

    /// TypeScript `backgroundTasks(toolUseId)`: `backgrounded`, `true` when the CLI leaves it out. A response
    /// without an object, or a `backgrounded` that is not a boolean, is a protocol error here, where TypeScript
    /// throws or returns the value untyped.
    pub async fn background_tasks(&self, tool_use_id: Option<crate::ToolUseId>) -> Result<bool, Error> {
        let response = self
            .control(
                ControlOperation::BackgroundTasks,
                RequestPayload::BackgroundTasks {
                    tool_use_id: tool_use_id.as_ref().map(crate::ToolUseId::as_str),
                }
                .to_value()?,
            )
            .await?;
        let Some(serde_json::Value::Object(response)) = response else {
            return Err(missing_response(ControlOperation::BackgroundTasks));
        };
        match response.get("backgrounded") {
            None | Some(serde_json::Value::Null) => Ok(true),
            Some(serde_json::Value::Bool(backgrounded)) => Ok(*backgrounded),
            Some(_) => Err(ProtocolError::from(<serde_json::Error as serde::de::Error>::custom(
                "background_tasks answered a backgrounded that is not a boolean",
            ))
            .into()),
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

    pub async fn abort(&self) -> Result<(), Error> {
        self.aborted.store(true, std::sync::atomic::Ordering::SeqCst);
        self.enqueue(Command::Abort).await
    }

    async fn finish_with(&self, completion: Completion) -> Result<(), Error> {
        let (response, receiver) = oneshot::channel();
        let command = Command::FinishInput { completion, response };
        match self.send_command(command).await {
            Err(_) => Err(Error::CommandQueueTimeout),
            // Upstream `endInput()` (TypeScript) and `end_input()` (Python) only close stdin and never fail, so ending
            // the input of a run that already stopped is a no-op; the run's own error surfaces from the message
            // stream.
            Ok(Err(_)) => Ok(()),
            Ok(Ok(())) => receiver.await.unwrap_or(Ok(())),
        }
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
        if self.aborted.load(std::sync::atomic::Ordering::SeqCst) {
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
