use crate::config::Config;
use crate::metrics::METRICS;
use crate::session::WasmSession;
use crate::terminal::{TerminalKind, WasmTerminal};
use crate::traits::{
    ChildProcessHandle, Clock, Fs, IdGenerator, NatsBroker, ProcessSpawner, Runtime,
    SemaphoreTaskLimiter, StdClock, StdSyncFs, TaskLimiter, TokioFs, TokioProcessSpawner,
    UuidGenerator, WasmExecutor, WasmRunConfig,
};
use crate::wasm::RealWasmExecutor;
use agent_client_protocol::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, TerminalExitStatus, TerminalId,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tracing::{debug, info};

/// Central execution runtime — one instance per process, shared across sessions.
///
/// Manages per-session sandbox directories and terminal processes.
/// The wasmtime `Engine` is pre-initialized for WASM module execution.
///
/// The default type parameters (`TokioFs`, `TokioProcessSpawner`) use the real
/// OS filesystem and process APIs. Pass `MockFs` / `MockProcessSpawner` in
/// tests to avoid touching disk or spawning real processes.
pub struct WasmRuntime<
    FS = TokioFs,
    PS = TokioProcessSpawner,
    N = async_nats::Client,
    WE = RealWasmExecutor<async_nats::Client, StdClock, StdSyncFs>,
    CL = StdClock,
    IG = UuidGenerator,
    TL = SemaphoreTaskLimiter,
> where
    FS: Fs,
    PS: ProcessSpawner,
    N: NatsBroker + Send + Sync,
    WE: WasmExecutor<N>,
    CL: Clock,
    IG: IdGenerator,
    TL: TaskLimiter,
{
    /// Filesystem abstraction for session sandbox operations.
    fs: FS,
    /// Process spawner abstraction for native terminal creation.
    process_spawner: PS,
    /// WASM executor abstraction for module compilation and execution.
    wasm_executor: WE,
    /// Root directory under which per-session sandboxes are created.
    session_root: PathBuf,
    /// Maximum output bytes retained per terminal.
    output_byte_limit: usize,
    /// When `true`, the first permission option is auto-selected.
    auto_allow_permissions: bool,
    /// Optional wall-clock timeout for WASM module execution.
    wasm_timeout_secs: Option<u64>,
    /// When `true`, reject commands that are not `.wasm` files.
    wasm_only: bool,
    /// When `true`, WASM modules can access the network via WASI sockets.
    wasm_allow_network: bool,
    /// Live sessions keyed by ACP session_id.
    sessions: RefCell<HashMap<String, WasmSession>>,
    /// All terminals across all sessions, keyed by terminal_id.
    terminals: RefCell<HashMap<String, WasmTerminal<PS::Handle>>>,
    /// Maps session_id → set of terminal_ids for session-level auto-cleanup.
    session_terminals: RefCell<HashMap<String, HashSet<String>>>,
    /// Reverse mapping terminal_id → session_id (for O(1) session lookup on terminal ops).
    terminal_session: RefCell<HashMap<String, String>>,
    /// Optional NATS client for trogon host functions in WASM modules.
    nats_client: Option<N>,
    /// Optional memory limit for WASM module execution in bytes.
    wasm_memory_limit_bytes: Option<usize>,
    /// Maximum fuel (instruction budget) for a single WASM module execution.
    wasm_fuel_limit: u64,
    /// Maximum number of trogon.* host function calls per WASM module execution.
    wasm_host_call_limit: u32,
    /// ACP subject prefix for WASM permission requests over NATS.
    acp_prefix: String,
    /// Task limiter controlling concurrent WASM task spawns.
    task_limiter: TL,
    /// How long (in seconds) a session can be idle before automatic cleanup.
    /// 0 means disabled.
    session_idle_timeout_secs: u64,
    /// Maximum seconds to wait for a terminal to exit before killing it. 0 means no timeout.
    wait_for_exit_timeout_secs: u64,
    /// Clock abstraction for session idle-timeout tracking.
    clock: CL,
    /// ID generator for terminal IDs and atomic temp-file names.
    id_gen: IG,
}

/// Constructors that default to the real OS filesystem, process spawner, clock, and ID generator.
impl WasmRuntime<TokioFs, TokioProcessSpawner, async_nats::Client> {
    pub fn new(config: &Config) -> Result<Self, wasmtime::Error> {
        Self::with_nats(config, None)
    }

    pub fn with_nats(
        config: &Config,
        nats_client: Option<async_nats::Client>,
    ) -> Result<Self, wasmtime::Error> {
        let wasm_executor = RealWasmExecutor::new(config)?;
        let permits = if config.wasm_max_concurrent_tasks == 0 {
            tokio::sync::Semaphore::MAX_PERMITS
        } else {
            config.wasm_max_concurrent_tasks.max(1)
        };
        Self::with_services(
            config,
            nats_client,
            TokioFs,
            TokioProcessSpawner,
            wasm_executor,
            StdClock,
            UuidGenerator,
            SemaphoreTaskLimiter::new(permits),
        )
    }
}

impl<FS, PS, N, WE, CL, IG, TL> WasmRuntime<FS, PS, N, WE, CL, IG, TL>
where
    FS: Fs,
    PS: ProcessSpawner,
    N: NatsBroker + Send + Sync,
    WE: WasmExecutor<N>,
    CL: Clock,
    IG: IdGenerator,
    TL: TaskLimiter,
{
    /// Creates a runtime with fully injectable dependencies.
    ///
    /// Production code uses [`WasmRuntime::new`] or [`WasmRuntime::with_nats`].
    #[allow(clippy::too_many_arguments)]
    pub fn with_services(
        config: &Config,
        nats_client: Option<N>,
        fs: FS,
        process_spawner: PS,
        wasm_executor: WE,
        clock: CL,
        id_gen: IG,
        task_limiter: TL,
    ) -> Result<Self, wasmtime::Error> {
        Ok(Self {
            fs,
            process_spawner,
            wasm_executor,
            session_root: config.session_root.clone(),
            output_byte_limit: config.output_byte_limit,
            auto_allow_permissions: config.auto_allow_permissions,
            wasm_timeout_secs: config.wasm_timeout_secs,
            wasm_only: config.wasm_only,
            wasm_allow_network: config.wasm_allow_network,
            sessions: RefCell::new(HashMap::new()),
            terminals: RefCell::new(HashMap::new()),
            session_terminals: RefCell::new(HashMap::new()),
            terminal_session: RefCell::new(HashMap::new()),
            nats_client,
            wasm_memory_limit_bytes: config.wasm_memory_limit_bytes,
            wasm_fuel_limit: config.wasm_fuel_limit,
            wasm_host_call_limit: config.wasm_host_call_limit,
            acp_prefix: config.acp_prefix.clone(),
            task_limiter,
            session_idle_timeout_secs: config.session_idle_timeout_secs,
            wait_for_exit_timeout_secs: config.wait_for_exit_timeout_secs,
            clock,
            id_gen,
        })
    }

    /// Returns (and lazily registers) the sandbox path for a session.
    ///
    /// Does **not** create the directory on disk — call [`ensure_session_dir`] for that.
    fn session_dir(&self, session_id: &str) -> PathBuf {
        let mut sessions = self.sessions.borrow_mut();
        if let Some(s) = sessions.get_mut(session_id) {
            s.last_activity = self.clock.now();
            return s.dir.clone();
        }
        let dir = self.session_root.join(session_id);
        sessions.insert(
            session_id.to_string(),
            WasmSession::new(dir.clone(), self.clock.now()),
        );
        dir
    }

    /// Updates the last_activity timestamp for a session (called on every access).
    pub fn tick_last_activity(&self, session_id: &str) {
        if let Some(s) = self.sessions.borrow_mut().get_mut(session_id) {
            s.last_activity = self.clock.now();
        }
    }

    /// Ticks last_activity for whichever session owns the given terminal.
    fn tick_last_activity_for_terminal(&self, terminal_id: &str) {
        if let Some(sid) = self.terminal_session.borrow().get(terminal_id).cloned() {
            self.tick_last_activity(&sid);
        }
    }

    /// Cleans up sessions that have been idle longer than `session_idle_timeout_secs`.
    pub fn cleanup_idle_sessions(&self) {
        if self.session_idle_timeout_secs == 0 {
            return;
        }
        let now = self.clock.now();
        let timeout = std::time::Duration::from_secs(self.session_idle_timeout_secs);
        let idle: Vec<String> = {
            let sessions = self.sessions.borrow();
            sessions
                .iter()
                .filter(|(_, s)| now.duration_since(s.last_activity) > timeout)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in idle {
            // Use a local runtime handle to drive the async cleanup.
            // Since cleanup_session is async we schedule it as a local task.
            let session_id = id.clone();
            // Remove session_terminals and terminal_session for this session;
            // clean up synchronously what we can (session map entry).
            // Full async cleanup (fs removal) requires an async context.
            // We perform a best-effort sync cleanup here.
            let terminal_ids: Vec<String> = self
                .session_terminals
                .borrow_mut()
                .remove(&session_id)
                .unwrap_or_default()
                .into_iter()
                .collect();
            for tid in terminal_ids {
                self.terminal_session.borrow_mut().remove(&tid);
                if let Some(mut t) = self.terminals.borrow_mut().remove(&tid) {
                    // Spawn kill + collector abort so native processes are not orphaned.
                    // cleanup_idle_sessions is sync, so async work must go to spawn_local.
                    tokio::task::spawn_local(async move {
                        t.kill().await;
                        if let Some(c) = t.output_collector.take() {
                            c.abort();
                        }
                    });
                }
            }
            if let Some(session) = self.sessions.borrow_mut().remove(&session_id) {
                let dir = session.dir.clone();
                let fs = self.fs.clone();
                // Spawn the fs removal as a background task if we're in an async context.
                tokio::task::spawn_local(async move {
                    let _ = fs.remove_dir_all(&dir).await;
                    debug!(session_id = %session_id, "Idle session sandbox cleaned up");
                });
            }
        }
    }

    /// Cleans up all active sessions (called at graceful shutdown).
    pub async fn cleanup_all_sessions(&self) {
        let session_ids: Vec<String> = self.sessions.borrow().keys().cloned().collect();
        for sid in session_ids {
            self.cleanup_session(&sid).await;
        }
    }

    /// Removes any leftover session sandbox directories from a previous run.
    /// Called once at startup before accepting any requests.
    pub async fn cleanup_stale_sessions(&self) {
        let root = &self.session_root;
        let dirs = match self.fs.list_subdirs(root).await {
            Ok(d) => d,
            Err(_) => return, // root doesn't exist yet, nothing to clean
        };
        let mut cleaned = 0u32;
        for path in dirs {
            if self.fs.remove_dir_all(&path).await.is_ok() {
                cleaned += 1;
            }
        }
        if cleaned > 0 {
            tracing::info!(
                count = cleaned,
                root = %root.display(),
                "Cleaned up stale session directories from previous run"
            );
        }
    }

    /// Ensures the sandbox directory for a session exists on disk.
    async fn ensure_session_dir(&self, session_id: &str) -> std::io::Result<PathBuf> {
        let dir = self.session_dir(session_id);
        self.fs.create_dir_all(&dir).await?;
        // Create the tmp sub-directory so that TMPDIR is usable for native processes.
        self.fs.create_dir_all(&dir.join("tmp")).await?;
        Ok(dir)
    }

    // ── Terminal operations ────────────────────────────────────────────────

    pub async fn handle_create_terminal(
        &self,
        session_id: &str,
        req: CreateTerminalRequest,
    ) -> agent_client_protocol::Result<CreateTerminalResponse> {
        let sandbox_dir = self
            .ensure_session_dir(session_id)
            .await
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        let cwd = {
            let sessions = self.sessions.borrow();
            sessions
                .get(session_id)
                .map(|s| s.terminal_cwd(req.cwd.as_deref()))
                .unwrap_or_else(|| sandbox_dir.clone())
        };

        // Ensure the cwd exists inside the sandbox.
        self.fs
            .create_dir_all(&cwd)
            .await
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        let command = &req.command;
        let args = &req.args;

        // Determine if this is a WASM module (`.wasm` extension).
        let is_wasm = command.ends_with(".wasm");

        if is_wasm {
            return self
                .run_wasm_terminal(session_id, command, args, &req.env, &cwd, &sandbox_dir)
                .await;
        }

        if self.wasm_only {
            return Err(agent_client_protocol::Error::new(
                -32602,
                format!("native process spawning is disabled (wasm_only=true); command must be a .wasm file: {command}"),
            ));
        }

        // Native process — sandboxed to session directory.
        METRICS
            .native_tasks_started
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let output_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let was_truncated_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Build the env pairs to forward from the request.
        let env_pairs: Vec<(String, String)> = req
            .env
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect();

        let mut handle = self
            .process_spawner
            .spawn(
                command,
                args,
                &env_pairs,
                &cwd,
                &cwd,
                &sandbox_dir.join("tmp"),
            )
            .await
            .map_err(|e| {
                agent_client_protocol::Error::new(-32603, format!("Failed to spawn process: {e}"))
            })?;

        let stdin = handle.take_stdin();

        let terminal_id = self.id_gen.new_id();

        // Spawn background task to collect stdout.
        let buf_stdout = Arc::clone(&output_buf);
        let trunc_stdout = Arc::clone(&was_truncated_flag);
        let limit = self.output_byte_limit;
        let stdout_handle = if let Some(mut stdout) = handle.take_stdout() {
            tokio::task::spawn(async move {
                let mut chunk = [0u8; 4096];
                loop {
                    match stdout.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => crate::terminal::append_output(
                            &buf_stdout,
                            &trunc_stdout,
                            limit,
                            &chunk[..n],
                        ),
                        Err(_) => break,
                    }
                }
            })
        } else {
            tokio::task::spawn(async {})
        };

        // Spawn background task to collect stderr.
        let buf_stderr = Arc::clone(&output_buf);
        let trunc_stderr = Arc::clone(&was_truncated_flag);
        let stderr_handle = if let Some(mut stderr) = handle.take_stderr() {
            tokio::task::spawn(async move {
                let mut chunk = [0u8; 4096];
                loop {
                    match stderr.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => crate::terminal::append_output(
                            &buf_stderr,
                            &trunc_stderr,
                            limit,
                            &chunk[..n],
                        ),
                        Err(_) => break,
                    }
                }
            })
        } else {
            tokio::task::spawn(async {})
        };

        // Combined collector waits for both streams.
        let collector = tokio::task::spawn(async move {
            let _ = tokio::join!(stdout_handle, stderr_handle);
        });

        let terminal = WasmTerminal {
            kind: TerminalKind::Native {
                child: Some(handle),
                stdin,
            },
            output_buf,
            output_collector: Some(collector),
            exit_status: None,
            was_truncated: was_truncated_flag,
        };

        info!(session_id, terminal_id, command, "Terminal created");
        self.terminals
            .borrow_mut()
            .insert(terminal_id.clone(), terminal);

        // Track terminal → session mapping for session-level cleanup.
        self.session_terminals
            .borrow_mut()
            .entry(session_id.to_string())
            .or_default()
            .insert(terminal_id.clone());
        self.terminal_session
            .borrow_mut()
            .insert(terminal_id.clone(), session_id.to_string());

        Ok(CreateTerminalResponse::new(TerminalId::new(
            terminal_id.as_str(),
        )))
    }

    /// Runs a `.wasm` module as a background tokio task.
    ///
    /// Uses the compiled module cache to avoid re-compilation on repeated invocations.
    /// Applies `wasm_timeout_secs` if configured. Returns immediately — WASM execution
    /// continues in a background `spawn_local` task until the module exits or is killed.
    async fn run_wasm_terminal(
        &self,
        session_id: &str,
        command: &str,
        args: &[String],
        env_vars: &[agent_client_protocol::EnvVariable],
        cwd: &std::path::Path,
        sandbox_dir: &std::path::Path,
    ) -> agent_client_protocol::Result<CreateTerminalResponse> {
        // Resolve relative WASM paths within the session sandbox to prevent path
        // traversal. Absolute paths are operator-deployed binaries and are allowed.
        let wasm_path = {
            let p = PathBuf::from(command);
            if p.is_absolute() {
                p
            } else {
                let resolved = self
                    .sessions
                    .borrow()
                    .get(session_id)
                    .and_then(|s| s.resolve_path(&p));
                resolved.ok_or_else(|| {
                    agent_client_protocol::Error::new(
                        -32602,
                        format!("WASM module path '{command}' escapes session sandbox"),
                    )
                })?
            }
        };
        let env_pairs: Vec<(String, String)> = env_vars
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect();

        // Eagerly validate the WASM file (executor-specific pre-flight check) before
        // spawning the background task. This preserves the contract that `create_terminal`
        // returns an error synchronously when the module is missing or too large.
        // `MockWasmExecutor::validate` is a no-op; `RealWasmExecutor::validate` checks
        // file existence and size limits.
        self.wasm_executor
            .validate(&wasm_path)
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        // Shared output buffer and exit status arc for the background task.
        let output_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let was_truncated_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wasm_exit: Arc<std::sync::OnceLock<TerminalExitStatus>> =
            Arc::new(std::sync::OnceLock::new());

        let terminal_id = self.id_gen.new_id();

        // Clone what the background task needs.
        let buf = Arc::clone(&output_buf);
        let trunc = Arc::clone(&was_truncated_flag);
        let exit_arc = Arc::clone(&wasm_exit);
        let limit = self.output_byte_limit;
        let timeout = self.wasm_timeout_secs;
        let memory_limit = self.wasm_memory_limit_bytes;
        let nats_for_task = self.nats_client.clone();
        let allow_network = self.wasm_allow_network;
        let auto_allow = self.auto_allow_permissions;
        let fuel_limit = self.wasm_fuel_limit;
        let host_call_limit = self.wasm_host_call_limit;
        let acp_prefix = self.acp_prefix.clone();
        let task_limiter = self.task_limiter.clone();
        let executor = self.wasm_executor.clone();
        // Build full argv: command (wasm_path) as argv[0], then user-supplied args.
        let mut argv_cloned: Vec<String> = Vec::with_capacity(args.len() + 1);
        argv_cloned.push(command.to_string());
        argv_cloned.extend_from_slice(args);
        let sandbox_dir_cloned = sandbox_dir.to_path_buf();
        let cwd_cloned = cwd.to_path_buf();
        let session_id_cloned = session_id.to_string();
        let wasm_path_cloned = wasm_path.clone();

        let collector = tokio::task::spawn_local(async move {
            // Backpressure: acquire a permit before executing. The permit is held
            // for the lifetime of the task and released when dropped.
            let _permit = task_limiter.acquire().await;
            // Count tasks that actually start executing, not just those queued for a permit.
            METRICS
                .wasm_tasks_started
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let result = executor
                .run(WasmRunConfig {
                    wasm_path: wasm_path_cloned,
                    argv: argv_cloned,
                    env_vars: env_pairs,
                    sandbox_dir: sandbox_dir_cloned.clone(),
                    cwd: if cwd_cloned != sandbox_dir_cloned {
                        Some(cwd_cloned)
                    } else {
                        None
                    },
                    output_buf: Arc::clone(&buf),
                    was_truncated: Arc::clone(&trunc),
                    output_byte_limit: limit,
                    timeout_secs: timeout,
                    memory_limit_bytes: memory_limit,
                    nats_client: nats_for_task,
                    session_id: session_id_cloned,
                    allow_network,
                    auto_allow_permissions: auto_allow,
                    fuel_limit,
                    host_call_limit,
                    acp_prefix,
                })
                .await;
            let exit_status = match result {
                Ok(status) => status,
                Err(e) => TerminalExitStatus::new().signal(Some(format!("{e}"))),
            };
            // Update metrics based on how the task exited.
            if exit_status.signal.is_some() {
                METRICS
                    .wasm_tasks_faulted
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if exit_status.signal.as_deref() == Some("fuel_exhausted") {
                    METRICS
                        .wasm_fuel_exhausted
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                METRICS
                    .wasm_tasks_completed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = exit_arc.set(exit_status);
        });

        let terminal = WasmTerminal {
            kind: TerminalKind::Wasm {
                exit_arc: wasm_exit,
            },
            output_buf,
            output_collector: Some(collector),
            exit_status: None,
            was_truncated: was_truncated_flag,
        };

        info!(
            session_id,
            terminal_id, command, "WASM module executing (background)"
        );
        self.terminals
            .borrow_mut()
            .insert(terminal_id.clone(), terminal);

        // Track terminal → session mapping for session-level cleanup.
        self.session_terminals
            .borrow_mut()
            .entry(session_id.to_string())
            .or_default()
            .insert(terminal_id.clone());
        self.terminal_session
            .borrow_mut()
            .insert(terminal_id.clone(), session_id.to_string());

        Ok(CreateTerminalResponse::new(TerminalId::new(
            terminal_id.as_str(),
        )))
    }

    pub async fn handle_terminal_output(
        &self,
        req: TerminalOutputRequest,
    ) -> agent_client_protocol::Result<TerminalOutputResponse> {
        let terminal_id = req.terminal_id.0.as_ref().to_string();
        self.tick_last_activity_for_terminal(&terminal_id);
        let terminals = self.terminals.borrow();
        match terminals.get(&terminal_id) {
            Some(t) => Ok(t.snapshot()),
            None => Err(agent_client_protocol::Error::new(
                -32602,
                format!("Unknown terminal: {terminal_id}"),
            )),
        }
    }

    pub async fn handle_kill_terminal(
        &self,
        req: KillTerminalRequest,
    ) -> agent_client_protocol::Result<KillTerminalResponse> {
        let terminal_id = req.terminal_id.0.as_ref().to_string();
        self.tick_last_activity_for_terminal(&terminal_id);
        // Remove the terminal from the map so the borrow is dropped before the await.
        let t = self.terminals.borrow_mut().remove(&terminal_id);
        match t {
            Some(mut terminal) => {
                terminal.kill().await;
                // Put the terminal back so it can still be waited on / released.
                self.terminals.borrow_mut().insert(terminal_id, terminal);
                Ok(KillTerminalResponse::new())
            }
            None => Err(agent_client_protocol::Error::new(
                -32602,
                format!("Unknown terminal: {terminal_id}"),
            )),
        }
    }

    pub async fn handle_write_to_terminal(
        &self,
        terminal_id: &str,
        data: &[u8],
    ) -> agent_client_protocol::Result<()> {
        self.tick_last_activity_for_terminal(terminal_id);
        // Remove the terminal from the map so the borrow is dropped before the await.
        let t = self.terminals.borrow_mut().remove(terminal_id);
        match t {
            Some(mut terminal) => {
                let ok = terminal.write_stdin(data).await;
                self.terminals
                    .borrow_mut()
                    .insert(terminal_id.to_string(), terminal);
                if ok {
                    Ok(())
                } else {
                    Err(agent_client_protocol::Error::new(
                        -32603,
                        "Failed to write to terminal stdin (WASM terminals do not support stdin)"
                            .to_string(),
                    ))
                }
            }
            None => Err(agent_client_protocol::Error::new(
                -32602,
                format!("Unknown terminal: {terminal_id}"),
            )),
        }
    }

    /// Closes the stdin pipe of a terminal, sending EOF to the process.
    pub fn handle_close_terminal_stdin(
        &self,
        terminal_id: &str,
    ) -> agent_client_protocol::Result<()> {
        self.tick_last_activity_for_terminal(terminal_id);
        let mut terminals = self.terminals.borrow_mut();
        match terminals.get_mut(terminal_id) {
            Some(t) => {
                t.close_stdin();
                Ok(())
            }
            None => Err(agent_client_protocol::Error::new(
                -32602,
                format!("Unknown terminal: {terminal_id}"),
            )),
        }
    }

    /// Kills the terminal process (if still running), drops its output buffer, and
    /// removes it from the session's terminal set.
    ///
    /// **Session destruction**: when this is the last terminal in a session, the
    /// session sandbox directory is deleted from disk. Any files written via
    /// `write_text_file` are permanently lost once all terminals are released.
    pub async fn handle_release_terminal(
        &self,
        req: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        let terminal_id = req.terminal_id.0.as_ref().to_string();
        self.tick_last_activity_for_terminal(&terminal_id);

        // Look up the owning session before removing the terminal.
        let maybe_session_id = self.terminal_session.borrow().get(&terminal_id).cloned();

        let t = self.terminals.borrow_mut().remove(&terminal_id);
        match t {
            Some(mut t) => {
                // The RefCell borrow is already released above; safe to await.
                t.kill().await;
                if let Some(collector) = t.output_collector.take() {
                    collector.abort();
                }
                debug!(terminal_id, "Terminal released");

                // Clean up tracking maps and trigger session cleanup if this was the last terminal.
                if let Some(sid) = maybe_session_id {
                    self.terminal_session.borrow_mut().remove(&terminal_id);
                    let should_cleanup = {
                        let mut st = self.session_terminals.borrow_mut();
                        if let Some(ids) = st.get_mut(&sid) {
                            ids.remove(&terminal_id);
                            ids.is_empty()
                        } else {
                            false
                        }
                    };
                    if should_cleanup {
                        self.cleanup_session(&sid).await;
                    }
                }

                Ok(ReleaseTerminalResponse::new())
            }
            None => Err(agent_client_protocol::Error::new(
                -32602,
                format!("Unknown terminal: {terminal_id}"),
            )),
        }
    }

    pub async fn handle_wait_for_terminal_exit(
        &self,
        req: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        let terminal_id = req.terminal_id.0.as_ref().to_string();
        self.tick_last_activity_for_terminal(&terminal_id);

        // Fast path: return cached exit status without touching the terminal.
        {
            let terminals = self.terminals.borrow();
            match terminals.get(&terminal_id) {
                Some(t) => {
                    if let Some(ref cached) = t.exit_status {
                        return Ok(WaitForTerminalExitResponse::new(cached.clone()));
                    }
                }
                None => {
                    return Err(agent_client_protocol::Error::new(
                        -32602,
                        format!("Unknown terminal: {terminal_id}"),
                    ));
                }
            }
        }

        // Extract the async pieces while keeping the terminal in the map so that
        // concurrent `terminal_output` calls keep working during the wait.
        let (collector, child, wasm_exit_arc) = {
            let mut terminals = self.terminals.borrow_mut();
            match terminals.get_mut(&terminal_id) {
                Some(t) => {
                    // Double-check: may have been set between the two borrows.
                    if let Some(ref cached) = t.exit_status {
                        return Ok(WaitForTerminalExitResponse::new(cached.clone()));
                    }
                    let (child, wasm_exit_arc) = match &mut t.kind {
                        TerminalKind::Native { child, .. } => (child.take(), None),
                        TerminalKind::Wasm { exit_arc } => {
                            (None, Some(std::sync::Arc::clone(exit_arc)))
                        }
                    };
                    (t.output_collector.take(), child, wasm_exit_arc)
                }
                None => {
                    return Err(agent_client_protocol::Error::new(
                        -32602,
                        format!("Unknown terminal: {terminal_id}"),
                    ));
                }
            }
        };

        // Wait outside any borrow — terminal stays in map the whole time.
        let timeout = self.wait_for_exit_timeout_secs;
        // Keep child_opt in scope (not moved into a future) so that on timeout
        // we can call child.kill() via the safe Tokio API instead of libc::kill
        // by PID, which has a PID-reuse race if the process exits just as the
        // timeout fires.
        let mut child_opt = child;

        let status = if let Some(ref mut child_proc) = child_opt {
            // Native process: wait for exit with optional kill-on-timeout.
            // child_proc.wait() borrows child_proc for the duration of the future;
            // when the timeout drops the future the borrow ends and child_proc is
            // accessible again in the Err arm for the safe kill() call.
            // stdout/stderr are already drained by the background collector task,
            // so wait() (not wait_with_output()) is sufficient here.
            if timeout == 0 {
                let s = match child_proc.wait().await {
                    Ok(st) => crate::terminal::exit_status_from_std(&st),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to wait for terminal process");
                        TerminalExitStatus::new()
                    }
                };
                if let Some(c) = collector {
                    let _ = c.await;
                }
                s
            } else {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    child_proc.wait(),
                )
                .await
                {
                    Ok(Ok(exit_status)) => {
                        if let Some(c) = collector {
                            let _ = c.await;
                        }
                        crate::terminal::exit_status_from_std(&exit_status)
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "Failed to wait for terminal process");
                        if let Some(c) = collector {
                            c.abort();
                        }
                        TerminalExitStatus::new()
                    }
                    Err(_elapsed) => {
                        tracing::warn!(
                            terminal_id,
                            "Terminal wait timed out after {timeout}s — killing"
                        );
                        // child_proc.wait() future was dropped by tokio::time::timeout,
                        // releasing its borrow; child_proc is now accessible for kill().
                        let _ = child_proc.kill().await;
                        if let Some(c) = collector {
                            c.abort();
                        }
                        TerminalExitStatus::new().signal(Some("wait_timeout".to_string()))
                    }
                }
            }
        } else if let Some(c) = collector {
            // WASM background task: wait for the task to finish.
            let _ = c.await;
            wasm_exit_arc
                .as_ref()
                .and_then(|arc| arc.get().cloned())
                .unwrap_or_else(TerminalExitStatus::new)
        } else {
            // Both child and collector are None. Two cases:
            //   a) Terminal already exited — the fast-path above would have caught this;
            //      we only reach here on a TOCTOU race between the fast-path check and
            //      the borrow_mut extraction (harmless; wasm_exit_arc will have a value).
            //   b) Another wait_for_terminal_exit call is running and already took the
            //      collector/child. For WASM terminals, poll the shared exit arc until
            //      the background task (and the other caller) populate it. For native
            //      terminals there is no shared arc — return empty (programming error:
            //      two concurrent waits on the same native terminal is unsupported).
            if let Some(arc) = wasm_exit_arc {
                loop {
                    if let Some(status) = arc.get().cloned() {
                        break status;
                    }
                    tokio::task::yield_now().await;
                }
            } else {
                tracing::warn!(
                    terminal_id = %terminal_id,
                    "Concurrent wait_for_terminal_exit on native terminal — \
                     second caller gets empty status; concurrent native waits are unsupported"
                );
                TerminalExitStatus::new()
            }
        };

        // Cache the exit status back so subsequent calls and snapshots see it.
        {
            let mut terminals = self.terminals.borrow_mut();
            if let Some(t) = terminals.get_mut(&terminal_id) {
                t.exit_status = Some(status.clone());
            }
        }

        Ok(WaitForTerminalExitResponse::new(status))
    }

    /// Cleans up all state for a session after all its terminals are released.
    async fn cleanup_session(&self, session_id: &str) {
        // Remove any remaining terminal tracking entries for this session
        // (should be empty at this point, but be defensive).
        let terminal_ids: Vec<String> = self
            .session_terminals
            .borrow_mut()
            .remove(session_id)
            .unwrap_or_default()
            .into_iter()
            .collect();

        for tid in terminal_ids {
            self.terminal_session.borrow_mut().remove(&tid);
            // Remove from the map and immediately drop the RefMut borrow before awaiting.
            let terminal = self.terminals.borrow_mut().remove(&tid);
            if let Some(mut t) = terminal {
                t.kill().await;
                if let Some(c) = t.output_collector.take() {
                    c.abort();
                }
            }
        }

        // Delete the sandbox directory and free session state.
        // Clone the dir path out so the RefMut borrow is dropped before the await.
        let session_dir = self.sessions.borrow_mut().remove(session_id).map(|s| s.dir);
        if let Some(dir) = session_dir {
            let _ = self.fs.remove_dir_all(&dir).await;
            debug!(session_id, "Session sandbox cleaned up");
        }
    }

    // ── Filesystem operations ──────────────────────────────────────────────

    pub async fn handle_write_text_file(
        &self,
        session_id: &str,
        req: WriteTextFileRequest,
    ) -> agent_client_protocol::Result<WriteTextFileResponse> {
        self.ensure_session_dir(session_id)
            .await
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        let dest = {
            let sessions = self.sessions.borrow();
            sessions
                .get(session_id)
                .and_then(|s| s.resolve_path(&req.path))
        };

        let dest = dest.ok_or_else(|| {
            agent_client_protocol::Error::new(
                -32602,
                format!("Path {} is outside the session sandbox", req.path.display()),
            )
        })?;

        if let Some(parent) = dest.parent() {
            self.fs
                .create_dir_all(parent)
                .await
                .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;
        }

        // Write atomically: write to a temp file first, then rename.
        let tmp_dest = dest.with_file_name(format!(".{}.tmp", self.id_gen.new_id()));
        self.fs
            .write(&tmp_dest, req.content.as_bytes())
            .await
            .map_err(|e| {
                agent_client_protocol::Error::new(-32603, format!("Failed to write file: {e}"))
            })?;
        let tmp_dest_clone = tmp_dest.clone();
        let fs = self.fs.clone();
        self.fs.rename(&tmp_dest, &dest).await.map_err(|e| {
            // Clean up temp file on failure (best-effort).
            let fs2 = fs.clone();
            tokio::task::spawn_local(async move {
                let _ = fs2.remove_file(&tmp_dest_clone).await;
            });
            agent_client_protocol::Error::new(-32603, format!("Failed to finalize file write: {e}"))
        })?;

        debug!(
            session_id,
            path = %dest.display(),
            bytes = req.content.len(),
            "File written"
        );
        Ok(WriteTextFileResponse::new())
    }

    pub async fn handle_read_text_file(
        &self,
        session_id: &str,
        req: ReadTextFileRequest,
    ) -> agent_client_protocol::Result<ReadTextFileResponse> {
        self.ensure_session_dir(session_id)
            .await
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        let src = {
            let sessions = self.sessions.borrow();
            sessions
                .get(session_id)
                .and_then(|s| s.resolve_path(&req.path))
        };

        let src = src.ok_or_else(|| {
            agent_client_protocol::Error::new(
                -32602,
                format!("Path {} is outside the session sandbox", req.path.display()),
            )
        })?;

        let content = self
            .fs
            .read_to_string(&src)
            .await
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        Ok(ReadTextFileResponse::new(content))
    }

    // ── Permission ─────────────────────────────────────────────────────────

    pub fn handle_request_permission(
        &self,
        req: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        if self.auto_allow_permissions {
            // Select the first available option.
            if let Some(first) = req.options.first() {
                let outcome = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    first.option_id.clone(),
                ));
                return Ok(RequestPermissionResponse::new(outcome));
            }
        }
        // No options or permissions disabled → cancel.
        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ))
    }

    /// Returns a snapshot of all active session IDs.
    pub fn list_sessions(&self) -> Vec<String> {
        self.sessions.borrow().keys().cloned().collect()
    }

    /// Returns a snapshot of all terminal IDs and their session IDs.
    pub fn list_terminals(&self) -> Vec<(String, String)> {
        self.terminal_session
            .borrow()
            .iter()
            .map(|(tid, sid)| (tid.clone(), sid.clone()))
            .collect()
    }

    // ── Session notification ───────────────────────────────────────────────

    pub fn handle_session_notification(&self, notif: SessionNotification) {
        // Tick last_activity so that the idle-session cleanup does not evict
        // sessions that are actively receiving notifications from the agent.
        self.tick_last_activity(notif.session_id.0.as_ref());
        // The agent already published this to NATS via NatsClientProxy.
        // IDE clients subscribe to the session update subject directly.
        // We log it here for observability only.
        debug!(
            session_id = %notif.session_id,
            update_type = ?std::mem::discriminant(&notif.update),
            "Session notification received (passthrough)"
        );
    }
}

impl<FS, PS, N, WE, CL, IG, TL> Runtime for WasmRuntime<FS, PS, N, WE, CL, IG, TL>
where
    FS: Fs,
    PS: ProcessSpawner,
    N: NatsBroker + Send + Sync,
    WE: WasmExecutor<N>,
    CL: Clock,
    IG: IdGenerator,
    TL: TaskLimiter,
{
    async fn handle_create_terminal(
        &self,
        session_id: &str,
        req: agent_client_protocol::CreateTerminalRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::CreateTerminalResponse> {
        WasmRuntime::handle_create_terminal(self, session_id, req).await
    }

    async fn handle_terminal_output(
        &self,
        req: agent_client_protocol::TerminalOutputRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::TerminalOutputResponse> {
        WasmRuntime::handle_terminal_output(self, req).await
    }

    async fn handle_kill_terminal(
        &self,
        req: agent_client_protocol::KillTerminalRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::KillTerminalResponse> {
        WasmRuntime::handle_kill_terminal(self, req).await
    }

    async fn handle_write_to_terminal(
        &self,
        terminal_id: &str,
        data: &[u8],
    ) -> agent_client_protocol::Result<()> {
        WasmRuntime::handle_write_to_terminal(self, terminal_id, data).await
    }

    fn handle_close_terminal_stdin(&self, terminal_id: &str) -> agent_client_protocol::Result<()> {
        WasmRuntime::handle_close_terminal_stdin(self, terminal_id)
    }

    async fn handle_release_terminal(
        &self,
        req: agent_client_protocol::ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::ReleaseTerminalResponse> {
        WasmRuntime::handle_release_terminal(self, req).await
    }

    async fn handle_wait_for_terminal_exit(
        &self,
        req: agent_client_protocol::WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::WaitForTerminalExitResponse> {
        WasmRuntime::handle_wait_for_terminal_exit(self, req).await
    }

    async fn handle_write_text_file(
        &self,
        session_id: &str,
        req: agent_client_protocol::WriteTextFileRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::WriteTextFileResponse> {
        WasmRuntime::handle_write_text_file(self, session_id, req).await
    }

    async fn handle_read_text_file(
        &self,
        session_id: &str,
        req: agent_client_protocol::ReadTextFileRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::ReadTextFileResponse> {
        WasmRuntime::handle_read_text_file(self, session_id, req).await
    }

    fn handle_request_permission(
        &self,
        req: agent_client_protocol::RequestPermissionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::RequestPermissionResponse> {
        WasmRuntime::handle_request_permission(self, req)
    }

    fn handle_session_notification(&self, notif: agent_client_protocol::SessionNotification) {
        WasmRuntime::handle_session_notification(self, notif)
    }

    fn list_sessions(&self) -> Vec<String> {
        WasmRuntime::list_sessions(self)
    }

    fn list_terminals(&self) -> Vec<(String, String)> {
        WasmRuntime::list_terminals(self)
    }

    fn cleanup_idle_sessions(&self) {
        WasmRuntime::cleanup_idle_sessions(self)
    }

    async fn cleanup_all_sessions(&self) {
        WasmRuntime::cleanup_all_sessions(self).await
    }
}
