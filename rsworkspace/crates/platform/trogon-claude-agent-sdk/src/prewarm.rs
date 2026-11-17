//! `startup()` and `prewarm()` of `sdk.mjs` 0.3.323. Python 0.2.181 has neither, so the Rust API follows
//! TypeScript.
//!
//! `startup` returns a [`WarmQuery`]: a process that finished its initialize handshake and waits for a prompt.
//! `prewarm` returns a [`SpareProcess`]: a process started with `--await-claim`, parked in a directory of its own,
//! that [`SpareProcess::claim`] binds to a session later.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use tokio::sync::watch;
use unicode_normalization::UnicodeNormalization;

use crate::{
    AgentDefinition, AgentName, ClaudeAgentOptions, ClaudeSdkClient, ClientHandle, ControlOperation, Error,
    ExtraArgument, JsonObject, OptionError, PermissionMode, Query, UserInput, WorkspaceTrust,
    adapters::{
        CurrentDirectory, EnvironmentVariable, RuntimeAdapters, SessionFileSystem, SpawnProcess, TemporaryDirectory,
    },
    client::ControlTicket,
    constants::{CLAIM_FOLLOWING_SETTINGS, CLAIM_HOOK_SETTINGS, STARTUP_DEFAULT_LIMIT},
    driver::Completion,
    process_options::EnvironmentMode,
    startup_timeout::{StartupHooksRunning, StartupSettings},
    wire::runtime_payload::RequestPayload,
};

/// What [`startup`] and [`prewarm`] take: TypeScript's `{options, initializeTimeoutMs}`.
#[derive(Clone, Default)]
pub struct StartupOptions {
    pub options: ClaudeAgentOptions,
    /// The most time to wait for the process to initialize. `None` is 60 seconds, and for [`startup`] the time in
    /// which a SessionStart or Setup hook is reported running is then not counted, for at most 10 minutes. A value
    /// counts throughout. [`prewarm`] counts throughout for the default too.
    pub initialize_timeout: Option<Duration>,
    /// TypeScript `onStartupHooksRunning`: told how many reported startup hooks are still running.
    pub on_startup_hooks_running: Option<StartupHooksRunning>,
}

impl StartupOptions {
    pub fn new(options: ClaudeAgentOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }
}

/// A process that finished its initialize handshake and waits for a prompt.
pub struct WarmQuery {
    client: ClaudeSdkClient,
}

impl WarmQuery {
    /// TypeScript `WarmQuery.query(prompt)`. Taking `self` is how Rust says the call "can only be called once".
    pub async fn query(self, prompt: impl Into<UserInput>) -> Result<Query, Error> {
        Box::pin(crate::client::start_query(
            self.client,
            prompt.into(),
            Completion::Run,
            crate::client::PromptShape::SdkMjs,
        ))
        .await
    }

    /// TypeScript `WarmQuery.close()`: discards the process without sending a prompt.
    pub fn close(self) {
        self.client.close();
    }

    /// TypeScript `WarmQuery[Symbol.asyncDispose]()`: closes and waits for the process to be cleaned up.
    pub async fn dispose(self) -> Result<(), Error> {
        self.client.disconnect().await
    }

    /// The initialize response the process answered with.
    pub async fn connection_info(&self) -> Result<crate::ConnectionInfo, Error> {
        self.client.handle().initialization_result().await
    }
}

/// TypeScript `startup()`, with the native process spawner.
pub async fn startup(params: StartupOptions) -> Result<WarmQuery, Error> {
    let adapters: RuntimeAdapters = RuntimeAdapters::default();
    start_warm(params, adapters, true).await
}

/// [`startup`] through caller-supplied adapters. The caller owns spawning, so no executable discovery runs.
pub async fn startup_with_adapters<E, P, F>(
    params: StartupOptions,
    adapters: RuntimeAdapters<E, P, F>,
) -> Result<WarmQuery, Error>
where
    E: EnvironmentVariable + CurrentDirectory<Error = std::io::Error> + TemporaryDirectory,
    P: SpawnProcess<Error = std::io::Error>,
    F: SessionFileSystem,
{
    start_warm(params, adapters, false).await
}

async fn start_warm<E, P, F>(
    params: StartupOptions,
    adapters: RuntimeAdapters<E, P, F>,
    discover: bool,
) -> Result<WarmQuery, Error>
where
    E: EnvironmentVariable + CurrentDirectory<Error = std::io::Error> + TemporaryDirectory,
    P: SpawnProcess<Error = std::io::Error>,
    F: SessionFileSystem,
{
    let StartupOptions {
        options,
        initialize_timeout,
        on_startup_hooks_running,
    } = params;
    let settings = StartupSettings {
        initialize_timeout,
        on_hooks_running: on_startup_hooks_running,
    };
    let client = Box::pin(ClaudeSdkClient::connect_startup(options, adapters, discover, settings)).await?;
    Ok(WarmQuery { client })
}

/// What the process answered the claim with.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimInfo {
    /// The canonical working directory.
    pub cwd: String,
    pub session_id: String,
    /// How long the spare was parked.
    pub parked_ms: Option<f64>,
    pub sdk_mcp_settled: bool,
}

/// Why a claim did not happen, or did not fully apply. The `Display` text is TypeScript's message.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("SpareProcess.claim() can only be called once")]
    AlreadyClaimed,
    #[error("SpareProcess.claim(): the spare was closed or has exited; start the session with query()")]
    Unavailable,
    #[error(
        "SpareProcess.claim(): hook settings ({}) cannot be applied at claim; pass them to prewarm() in Options.settings, where they are in force before any hook, as on a cold start. Nothing was sent: this spare can still be claimed without them.",
        .keys.join(", ")
    )]
    HookSettings { keys: Vec<String> },
    #[error("spare_exited: the spare process exited before it was claimed")]
    SpareExited,
    #[error("spare_closed: the spare was closed before it was claimed")]
    SpareClosed,
    /// The process refused the claim (`not_a_spare`, `env_key_not_claimable`, `cwd_not_found`,
    /// `project_settings_not_claimable`, `permission_mode_not_claimable`, `claim_failed`, ...). A prompt already
    /// sent gets the `not_claimed` error result, and the process exits.
    #[error("{}", reason_text(.0))]
    Refused(Arc<Error>),
    #[error(
        "settings_not_applied: {reason} \u{2014} the prompt was not sent and the process was closed; start the session with query()"
    )]
    SettingsNotApplied { reason: String, claim: ClaimInfo },
    /// The prompt was sent and the session runs without these options.
    #[error(
        "option_not_applied: {} \u{2014} the prompt was sent and the session runs without these options",
        .reasons.join("; ")
    )]
    OptionNotApplied { reasons: Vec<String>, claim: ClaimInfo },
    /// Something this process failed at before or while writing: the working directory, an agent definition, or
    /// the prompt.
    #[error("{}", reason_text(.0))]
    Local(Arc<Error>),
}

impl ClaimError {
    /// TypeScript's `telemetryMessage` of the errors that carry one.
    pub fn telemetry_message(&self) -> Option<&'static str> {
        match self {
            Self::HookSettings { .. } => Some("SpareProcess.claim(): hook settings cannot be applied at claim"),
            Self::SettingsNotApplied { .. } => Some("settings_not_applied"),
            Self::OptionNotApplied { .. } => Some("option_not_applied"),
            Self::Refused(error) if matches!(**error, Error::ControlRejected { .. }) => {
                Some("Claude Code control request failed (claim_session)")
            }
            _ => None,
        }
    }

    /// TypeScript's `.claim`: what the process answered, when it answered the claim itself.
    pub fn claim(&self) -> Option<&ClaimInfo> {
        match self {
            Self::SettingsNotApplied { claim, .. } | Self::OptionNotApplied { claim, .. } => Some(claim),
            _ => None,
        }
    }
}

/// The text TypeScript shows for a failed request: the CLI's own `error` string for a rejection, otherwise the
/// error's message.
fn reason_text(error: &Error) -> String {
    match error {
        Error::ControlRejected { rejection, .. } => rejection.message().to_owned(),
        other => other.typescript_message().unwrap_or_else(|| other.to_string()),
    }
}

/// `ClaimOptions`: the per-session half of the options.
#[derive(Clone, Debug)]
pub struct ClaimOptions {
    /// The session's working directory. A relative path resolves against this process; `~/...` is expanded by
    /// Claude Code.
    pub cwd: PathBuf,
    /// Per-session environment additions, in the order they are sent.
    pub env: Option<Vec<(String, String)>>,
    pub additional_directories: Option<Vec<String>>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    /// `None` leaves it out, `Some(None)` is TypeScript's `null`.
    pub max_thinking_tokens: Option<Option<u64>>,
    /// The flag-tier settings overlay, as `apply_flag_settings` takes it.
    pub settings: Option<JsonObject>,
    pub append_system_prompt: Option<String>,
    pub title: Option<String>,
    pub agents: Option<std::collections::BTreeMap<AgentName, AgentDefinition>>,
    /// A trust that is accepted needs an absolute directory, otherwise it is dropped with a warning.
    pub workspace_trust: Option<WorkspaceTrust>,
}

impl ClaimOptions {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            env: None,
            additional_directories: None,
            model: None,
            permission_mode: None,
            max_thinking_tokens: None,
            settings: None,
            append_system_prompt: None,
            title: None,
            agents: None,
            workspace_trust: None,
        }
    }
}

type Cleanup = Arc<dyn Fn() + Send + Sync>;
type CurrentDir = Arc<dyn Fn() -> std::io::Result<PathBuf> + Send + Sync>;

#[derive(Default)]
struct SpareState {
    claimed: bool,
    closed: bool,
    exited: bool,
}

struct Shared {
    handle: ClientHandle,
    state: Mutex<SpareState>,
    outcome: watch::Sender<Option<Result<ClaimInfo, ClaimError>>>,
    sdk_mcp_servers: Vec<String>,
    cleanup: Cleanup,
    current_dir: CurrentDir,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    /// Settles `claimed` once; later settlements change nothing.
    fn settle(&self, result: Result<ClaimInfo, ClaimError>) {
        self.outcome.send_if_modified(|slot| {
            if slot.is_some() {
                return false;
            }
            *slot = Some(result);
            true
        });
    }
}

/// A parked, fully started process that is not yet any session. Single use.
pub struct SpareProcess {
    shared: Arc<Shared>,
    client: Mutex<Option<ClaudeSdkClient>>,
}

/// TypeScript `prewarm()`, with the native process spawner.
pub async fn prewarm(params: StartupOptions) -> Result<SpareProcess, Error> {
    let adapters: RuntimeAdapters = RuntimeAdapters::default();
    start_spare(params, adapters, true).await
}

/// [`prewarm`] through caller-supplied adapters. The caller owns spawning, so no executable discovery runs.
pub async fn prewarm_with_adapters<E, P, F>(
    params: StartupOptions,
    adapters: RuntimeAdapters<E, P, F>,
) -> Result<SpareProcess, Error>
where
    E: EnvironmentVariable
        + CurrentDirectory<Error = std::io::Error>
        + TemporaryDirectory
        + Clone
        + Send
        + Sync
        + 'static,
    P: SpawnProcess<Error = std::io::Error>,
    F: SessionFileSystem + Clone,
{
    start_spare(params, adapters, false).await
}

async fn start_spare<E, P, F>(
    params: StartupOptions,
    adapters: RuntimeAdapters<E, P, F>,
    discover: bool,
) -> Result<SpareProcess, Error>
where
    E: EnvironmentVariable
        + CurrentDirectory<Error = std::io::Error>
        + TemporaryDirectory
        + Clone
        + Send
        + Sync
        + 'static,
    P: SpawnProcess<Error = std::io::Error>,
    F: SessionFileSystem + Clone,
{
    let StartupOptions {
        mut options,
        initialize_timeout,
        on_startup_hooks_running,
    } = params;
    if options.resume.is_some()
        || options.continue_conversation == crate::ContinueConversation::Yes
        || options.fork_session == crate::SessionFork::Fork
    {
        return Err(OptionError::new(
            "prewarm(): resume/continue/forkSession describe a session; a spare has none \u{2014} pass them to query() instead",
        )
        .into());
    }
    let parked = options.cwd.is_none();
    let directory = match options.cwd.clone() {
        Some(directory) => directory,
        None => park_directory(&options, &adapters.environment, &adapters.filesystem)?,
    };
    let filesystem = adapters.filesystem.clone();
    let cleanup: Cleanup = if parked {
        let directory = directory.clone();
        Arc::new(move || {
            let _ = filesystem.remove_dir_all(&directory);
        })
    } else {
        Arc::new(|| {})
    };
    if options.workspace_trust.take().is_some() {
        tracing::warn!(
            "prewarm(): workspaceTrust is a per-session attestation \u{2014} pass it to claim(); not sent to the spare"
        );
    }
    let sdk_mcp_servers: Vec<String> = options
        .sdk_mcp_servers
        .keys()
        .map(|name| name.as_str().to_owned())
        .collect();
    options.cwd = Some(directory);
    match options.extra_args.iter_mut().find(|extra| extra.flag == "await-claim") {
        Some(extra) => extra.value = None,
        None => options.extra_args.push(ExtraArgument {
            flag: "await-claim".into(),
            value: None,
        }),
    }
    let environment = adapters.environment.clone();
    let settings = StartupSettings {
        initialize_timeout: Some(initialize_timeout.unwrap_or(STARTUP_DEFAULT_LIMIT)),
        on_hooks_running: on_startup_hooks_running,
    };
    let client = match Box::pin(ClaudeSdkClient::connect_startup(options, adapters, discover, settings)).await {
        Ok(client) => client,
        Err(error) => {
            cleanup();
            return Err(error);
        }
    };
    let current_dir: CurrentDir = Arc::new(move || environment.current_dir());
    let shared = Arc::new(Shared {
        handle: client.handle(),
        state: Mutex::new(SpareState::default()),
        outcome: watch::channel(None).0,
        sdk_mcp_servers,
        cleanup,
        current_dir,
    });
    let watcher = Arc::clone(&shared);
    tokio::spawn(async move {
        watcher.handle.stopped().await;
        let unclaimed = {
            let mut state = locked(&watcher.state);
            state.exited = true;
            !state.claimed
        };
        if unclaimed {
            watcher.settle(Err(ClaimError::SpareExited));
        }
        (watcher.cleanup)();
    });
    Ok(SpareProcess {
        shared,
        client: Mutex::new(Some(client)),
    })
}

/// The directory a spare without `cwd` is parked in: `<config home>/spares/spare-<random>`.
fn park_directory<E, F>(options: &ClaudeAgentOptions, environment: &E, filesystem: &F) -> Result<PathBuf, Error>
where
    E: EnvironmentVariable,
    F: SessionFileSystem,
{
    let home_name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let mode = options.environment_mode.resolve(&options.env);
    let from_options = |name: &str| {
        (mode != EnvironmentMode::Inherit)
            .then(|| options.env.get(name))
            .flatten()
            .map(|value| value.expose().to_owned())
    };
    let from_parent = |name: &str| {
        (mode != EnvironmentMode::Replace)
            .then(|| environment.var_os(OsStr::new(name)))
            .flatten()
            .map(|value| value.to_string_lossy().into_owned())
    };
    let lookup = |name: &str| from_options(name).or_else(|| from_parent(name));
    let process_home = environment
        .var_os(OsStr::new(home_name))
        .map(|value| PathBuf::from(&value))
        .unwrap_or_default();
    let configured = if mode == EnvironmentMode::Inherit {
        // `Wt()`: the variable as it is, normalized.
        lookup("CLAUDE_CONFIG_DIR")
            .map(|value| PathBuf::from(value.nfc().collect::<String>()))
            .unwrap_or_else(|| process_home.join(".claude"))
    } else {
        let config = lookup("CLAUDE_CONFIG_DIR")
            .map(|value| value.trim().to_owned())
            .unwrap_or_default();
        if config.is_empty() {
            let home = lookup(home_name)
                .map(|value| value.trim().to_owned())
                .unwrap_or_default();
            if home.is_empty() {
                process_home.join(".claude")
            } else {
                Path::new(&home).join(".claude")
            }
        } else {
            PathBuf::from(config)
        }
    };
    let config_home = if configured.is_absolute() {
        configured
    } else {
        process_home.join(".claude")
    };
    if !config_home.is_absolute() {
        return Err(OptionError::new(
            "prewarm(): no absolute config home to park the spare under (neither CLAUDE_CONFIG_DIR nor HOME gives an absolute path); set one of them to an absolute path or pass options.cwd",
        )
        .into());
    }
    let spares = config_home.join("spares");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    filesystem.create_directory(&builder, &spares).map_err(Error::Io)?;
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    let mut attempts = 0_u8;
    loop {
        // `mkdtemp`: six characters of `[A-Za-z0-9]`.
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        // Bytes at or above 248 are dropped so that the 62 letters stay equally likely; the version and variant
        // bytes of a UUID are not random.
        let suffix: String = [uuid::Uuid::new_v4(), uuid::Uuid::new_v4()]
            .iter()
            .flat_map(|id| id.as_bytes().iter().enumerate())
            .filter(|(index, byte)| *index != 6 && *index != 8 && **byte < 248)
            .filter_map(|(_, byte)| ALPHABET.get(usize::from(*byte) % ALPHABET.len()))
            .take(6)
            .map(|letter| char::from(*letter))
            .collect();
        if suffix.len() < 6 {
            continue;
        }
        let path = spares.join(format!("spare-{suffix}"));
        match filesystem.create_directory(&builder, &path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempts < 8 => attempts += 1,
            Err(error) => return Err(Error::Io(error)),
        }
    }
}

/// `path.resolve` of a relative claim directory against the process directory; any other value is sent as given.
fn claim_directory(cwd: &Path, current: &dyn Fn() -> std::io::Result<PathBuf>) -> std::io::Result<String> {
    let text = cwd.to_string_lossy();
    let relative = !text.trim().is_empty()
        && !crate::workspace_trust::absolute_anywhere(&text)
        && text != "~"
        && !text.starts_with("~/");
    if !relative {
        return Ok(text.into_owned());
    }
    let mut resolved = current()?;
    for component in cwd.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            other => resolved.push(other.as_os_str()),
        }
    }
    Ok(resolved.to_string_lossy().into_owned())
}

/// The settings of a claim, split the way TypeScript sends them: what the claim needs in force first, and the keys
/// that follow it.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct SettingsSplit {
    /// Sent ahead of the claim.
    pub before: Option<JsonObject>,
    /// Sent behind the claim, in the order of [`CLAIM_FOLLOWING_SETTINGS`].
    pub after: Option<JsonObject>,
}

pub(crate) fn split_settings(settings: &JsonObject) -> SettingsSplit {
    let before: JsonObject = settings
        .iter()
        .filter(|(key, _)| !CLAIM_FOLLOWING_SETTINGS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let after: JsonObject = CLAIM_FOLLOWING_SETTINGS
        .iter()
        .filter_map(|key| settings.get(*key).map(|value| ((*key).to_owned(), value.clone())))
        .collect();
    SettingsSplit {
        before: (!before.is_empty()).then_some(before),
        after: (!after.is_empty()).then_some(after),
    }
}

pub(crate) fn hook_settings(settings: &JsonObject) -> Vec<String> {
    CLAIM_HOOK_SETTINGS
        .iter()
        .filter(|key| settings.contains_key(**key))
        .map(|key| (*key).to_owned())
        .collect()
}

/// A control request that was queued, or that could not be queued.
enum Started {
    Queued(ControlTicket),
    Failed(Error),
}

impl Started {
    async fn begin(
        handle: &ClientHandle,
        operation: ControlOperation,
        request: Result<serde_json::Value, Error>,
    ) -> Self {
        match request {
            Ok(request) => match handle.control_started(operation, request).await {
                Ok(ticket) => Self::Queued(ticket),
                Err(error) => Self::Failed(error),
            },
            Err(error) => Self::Failed(error),
        }
    }

    async fn answer(self) -> Result<Option<serde_json::Value>, Error> {
        match self {
            Self::Queued(ticket) => ticket.wait().await,
            Self::Failed(error) => Err(error),
        }
    }
}

fn payload(request: &RequestPayload<'_>) -> Result<serde_json::Value, Error> {
    request.to_value().map_err(Error::from)
}

impl SpareProcess {
    /// Settles once the process has answered the claim and the per-session settings sent with it.
    pub async fn claimed(&self) -> Result<ClaimInfo, ClaimError> {
        let mut outcome = self.shared.outcome.subscribe();
        match outcome.wait_for(Option::is_some).await {
            Ok(settled) => settled.clone().unwrap_or(Err(ClaimError::SpareClosed)),
            Err(_) => Err(ClaimError::SpareClosed),
        }
    }

    /// Settles when the process exits, claimed or not. A parked spare that dies should be replaced.
    pub async fn exited(&self) {
        self.shared.handle.stopped().await;
    }

    /// TypeScript `SpareProcess.close()`: terminates the process. Before a claim this discards the spare and
    /// [`SpareProcess::claimed`] fails; after one it closes the query.
    pub fn close(&self) {
        let claimed = {
            let mut state = locked(&self.shared.state);
            if state.closed {
                return;
            }
            state.closed = true;
            state.claimed
        };
        self.shared.handle.close();
        if !claimed {
            self.shared.settle(Err(ClaimError::SpareClosed));
        }
    }

    /// TypeScript `SpareProcess[Symbol.asyncDispose]()`: closes and waits for the process to exit.
    pub async fn dispose(&self) {
        self.close();
        self.exited().await;
    }

    /// TypeScript `SpareProcess.claim({prompt, options})`: binds the spare to a session and sends the first
    /// message. The claim, the per-session settings and the prompt are written in TypeScript's order; a claim with
    /// a `settings` overlay sends the prompt once the overlay is accepted. Can only be called once.
    pub async fn claim(&self, prompt: impl Into<UserInput>, options: ClaimOptions) -> Result<Query, ClaimError> {
        let shared = &self.shared;
        {
            let state = locked(&shared.state);
            if state.claimed {
                return Err(ClaimError::AlreadyClaimed);
            }
            if state.closed || state.exited {
                return Err(ClaimError::Unavailable);
            }
        }
        if let Some(settings) = &options.settings {
            let keys = hook_settings(settings);
            if !keys.is_empty() {
                return Err(ClaimError::HookSettings { keys });
            }
        }
        let local = |error: Error| ClaimError::Local(Arc::new(error));
        let cwd = claim_directory(&options.cwd, &*shared.current_dir).map_err(|error| local(Error::Io(error)))?;
        let claim = claim_request(&cwd, &options, &shared.sdk_mcp_servers).map_err(local)?;
        let ClaimOptions {
            model,
            max_thinking_tokens,
            settings,
            ..
        } = options;
        let split = settings.as_ref().map(split_settings);
        let Some(client) = locked(&self.client).take() else {
            return Err(ClaimError::Unavailable);
        };
        {
            let mut state = locked(&shared.state);
            // A close between the check above and here wins.
            if state.closed || state.exited {
                *locked(&self.client) = Some(client);
                return Err(ClaimError::Unavailable);
            }
            state.claimed = true;
        }
        let handle = shared.handle.clone();
        handle.set_claim_pending(true);
        let before = match split.as_ref().and_then(|split| split.before.as_ref()) {
            Some(before) => Some(
                Started::begin(
                    &handle,
                    ControlOperation::ApplyFlagSettings,
                    payload(&RequestPayload::ApplyFlagSettings { settings: before }),
                )
                .await,
            ),
            None => None,
        };
        let claim = Started::begin(
            &handle,
            ControlOperation::ClaimSession,
            Ok(serde_json::Value::Object(claim)),
        )
        .await;
        let mut following = Vec::new();
        if let Some(model) = &model {
            following.push((
                "model",
                Started::begin(
                    &handle,
                    ControlOperation::SetModel,
                    payload(&RequestPayload::SetModel {
                        model: Some(Some(model.as_str())),
                    }),
                )
                .await,
            ));
        }
        if let Some(tokens) = max_thinking_tokens {
            following.push((
                "maxThinkingTokens",
                Started::begin(
                    &handle,
                    ControlOperation::SetMaxThinkingTokens,
                    payload(&RequestPayload::SetMaxThinkingTokens {
                        max_thinking_tokens: tokens,
                        thinking_display: None,
                    }),
                )
                .await,
            ));
        }
        let after = match split.as_ref().and_then(|split| split.after.as_ref()) {
            Some(after) => Some(
                Started::begin(
                    &handle,
                    ControlOperation::ApplyFlagSettings,
                    payload(&RequestPayload::ApplyFlagSettings { settings: after }),
                )
                .await,
            ),
            None => None,
        };
        let held_prompt = settings.is_some();
        // `claim` hands the prompt to `WarmQuery.query`, which writes a string prompt as `query()` does.
        let prompt = prompt.into().into_query_prompt(true);
        let (now, later) = if held_prompt {
            (None, Some(prompt))
        } else {
            (Some(prompt), None)
        };
        let settling = Settling {
            shared: Arc::clone(shared),
            claim,
            following,
            settings: held_prompt.then(|| before.into_iter().chain(after).collect::<Vec<_>>()),
            prompt: later,
            requested_cwd: cwd,
        };
        tokio::spawn(settling.run());
        if let Some(prompt) = now
            && let Err(error) = send_prompt(&handle, prompt).await
        {
            handle.close();
            return Err(local(error));
        }
        Ok(client.into_query())
    }
}

async fn send_prompt(handle: &ClientHandle, prompt: UserInput) -> Result<(), Error> {
    handle.send_input(prompt).await?;
    // `sdk.mjs` treats the claim prompt as a single user turn, as `start_query` does for a TypeScript run.
    handle.finish_with(Completion::Turn).await
}

/// The answer to `claim_session` becomes the initialization result: its `initialize` replaces the earlier one.
struct Settling {
    shared: Arc<Shared>,
    claim: Started,
    following: Vec<(&'static str, Started)>,
    /// `Some` when the claim carries a settings overlay: the requests that apply it, possibly none.
    settings: Option<Vec<Started>>,
    prompt: Option<UserInput>,
    /// The `cwd` the claim request carried.
    requested_cwd: String,
}

impl Settling {
    async fn run(self) {
        let Self {
            shared,
            claim,
            following,
            settings,
            prompt,
            requested_cwd,
        } = self;
        let handle = shared.handle.clone();
        let response = match claim.answer().await {
            // A frame without `response` reads like an empty answer, as `r?.initialize` does.
            Ok(response) => response.unwrap_or_default(),
            Err(error) => {
                handle.set_claim_pending(false);
                // `endInput()`: the process that refused is let go and exits once it has answered.
                let _ = handle.finish_with(Completion::Run).await;
                shared.settle(Err(ClaimError::Refused(Arc::new(error))));
                return;
            }
        };
        handle.merge_claim_initialization(&response);
        handle.set_claim_pending(false);
        (shared.cleanup)();
        let info = ClaimInfo {
            // TypeScript `Ye.cwd ?? x`: the directory the claim asked for when the answer names none.
            cwd: response
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .map_or(requested_cwd, str::to_owned),
            session_id: response
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .map_or_else(String::new, str::to_owned),
            parked_ms: response.get("parked_ms").and_then(serde_json::Value::as_f64),
            sdk_mcp_settled: response
                .get("sdk_mcp_settled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        };
        let mut settings_failure: Option<String> = None;
        if let Some(requests) = settings {
            for request in requests {
                if let Err(error) = request.answer().await
                    && settings_failure.is_none()
                {
                    settings_failure = Some(reason_text(&error));
                }
            }
            let closed = locked(&shared.state).closed;
            if settings_failure.is_none()
                && !closed
                && let Some(prompt) = prompt
            {
                let _ = send_prompt(&handle, prompt).await;
            }
        }
        let mut reasons = Vec::new();
        for (name, request) in following {
            if let Err(error) = request.answer().await {
                reasons.push(format!("{name}: {}", reason_text(&error)));
            }
        }
        if let Some(reason) = settings_failure {
            locked(&shared.state).closed = true;
            handle.close();
            shared.settle(Err(ClaimError::SettingsNotApplied { reason, claim: info }));
        } else if reasons.is_empty() {
            shared.settle(Ok(info));
        } else {
            shared.settle(Err(ClaimError::OptionNotApplied { reasons, claim: info }));
        }
    }
}

/// The `claim_session` request, keys in TypeScript's order.
fn claim_request(cwd: &str, options: &ClaimOptions, sdk_mcp_servers: &[String]) -> Result<JsonObject, Error> {
    let mut request = JsonObject::new();
    request.insert("subtype".into(), "claim_session".into());
    request.insert("cwd".into(), cwd.into());
    if let Some(mode) = &options.permission_mode {
        request.insert("permission_mode".into(), mode.as_str().into());
    }
    if let Some(env) = &options.env {
        let env: JsonObject = env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone().into()))
            .collect();
        request.insert("env".into(), env.into());
    }
    if let Some(directories) = &options.additional_directories {
        request.insert("additional_directories".into(), directories.clone().into());
    }
    if let Some(prompt) = &options.append_system_prompt {
        request.insert("append_system_prompt".into(), prompt.clone().into());
    }
    if let Some(title) = &options.title {
        request.insert("title".into(), title.clone().into());
    }
    if let Some(agents) = &options.agents {
        let agents = agents
            .iter()
            .map(|(name, definition)| definition.to_wire_json().map(|value| (name.as_str().to_owned(), value)))
            .collect::<Result<JsonObject, _>>()
            .map_err(crate::ProtocolError::from)?;
        request.insert("agents".into(), agents.into());
    }
    if let Some(trust) = options.workspace_trust.as_ref().and_then(WorkspaceTrust::validated)
        && trust.accepted == Some(true)
    {
        request.insert("workspace_trust".into(), trust.to_object().into());
    }
    request.insert("sdk_mcp_servers".into(), sdk_mcp_servers.to_vec().into());
    request.insert("include_initialize".into(), true.into());
    Ok(request)
}

#[cfg(test)]
#[path = "prewarm_tests.rs"]
mod tests;
