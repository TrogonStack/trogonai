use crate::config::Config;
use crate::metrics::METRICS;
use crate::session::WasmSession;
use crate::terminal::WasmTerminal;
use crate::wasm;
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
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{debug, info};
use uuid::Uuid;
use wasmtime::{Engine, Module};

/// Central execution runtime — one instance per process, shared across sessions.
///
/// Manages per-session sandbox directories and terminal processes.
/// The wasmtime `Engine` is pre-initialized for WASM module execution.
pub struct WasmRuntime {
    /// Shared wasmtime engine (Cranelift JIT, reuses compiled module cache).
    pub engine: Engine,
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
    terminals: RefCell<HashMap<String, WasmTerminal>>,
    /// Feature 1: Cache of compiled WASM modules keyed by absolute path.
    /// Value is (module, source_mtime) for invalidation on file change.
    modules: RefCell<HashMap<PathBuf, (Module, std::time::SystemTime)>>,
    /// Optional directory for on-disk compiled module cache.
    module_cache_dir: Option<PathBuf>,
    /// Feature 3: Maps session_id → set of terminal_ids for auto-cleanup.
    session_terminals: RefCell<HashMap<String, HashSet<String>>>,
    /// Feature 3: Reverse mapping terminal_id → session_id.
    terminal_session: RefCell<HashMap<String, String>>,
    /// Optional NATS client for trogon host functions in WASM modules.
    nats_client: Option<async_nats::Client>,
    /// Optional memory limit for WASM module execution in bytes.
    wasm_memory_limit_bytes: Option<usize>,
    /// Maximum fuel (instruction budget) for a single WASM module execution.
    wasm_fuel_limit: u64,
    /// Maximum number of trogon.* host function calls per WASM module execution.
    wasm_host_call_limit: u32,
    /// ACP subject prefix for WASM permission requests over NATS.
    acp_prefix: String,
    /// Semaphore limiting concurrent WASM task spawns.
    task_semaphore: Arc<Semaphore>,
    /// How long (in seconds) a session can be idle before automatic cleanup.
    /// 0 means disabled.
    session_idle_timeout_secs: u64,
    /// Maximum allowed size for a `.wasm` module file in bytes. 0 means unlimited.
    wasm_max_module_size_bytes: usize,
    /// Maximum seconds to wait for a terminal to exit before killing it. 0 means no timeout.
    wait_for_exit_timeout_secs: u64,
}

impl WasmRuntime {
    pub fn new(config: &Config) -> Result<Self, wasmtime::Error> {
        Self::with_nats(config, None)
    }

    pub fn with_nats(
        config: &Config,
        nats_client: Option<async_nats::Client>,
    ) -> Result<Self, wasmtime::Error> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.async_support(true);
        wasm_config.consume_fuel(true);
        let engine = Engine::new(&wasm_config)?;
        let semaphore_permits = if config.wasm_max_concurrent_tasks == 0 {
            tokio::sync::Semaphore::MAX_PERMITS
        } else {
            config.wasm_max_concurrent_tasks.max(1)
        };
        Ok(Self {
            engine,
            session_root: config.session_root.clone(),
            output_byte_limit: config.output_byte_limit,
            auto_allow_permissions: config.auto_allow_permissions,
            wasm_timeout_secs: config.wasm_timeout_secs,
            wasm_only: config.wasm_only,
            wasm_allow_network: config.wasm_allow_network,
            sessions: RefCell::new(HashMap::new()),
            terminals: RefCell::new(HashMap::new()),
            modules: RefCell::new(HashMap::new()),
            session_terminals: RefCell::new(HashMap::new()),
            terminal_session: RefCell::new(HashMap::new()),
            nats_client,
            wasm_memory_limit_bytes: config.wasm_memory_limit_bytes,
            module_cache_dir: config.module_cache_dir.clone(),
            wasm_fuel_limit: config.wasm_fuel_limit,
            wasm_host_call_limit: config.wasm_host_call_limit,
            acp_prefix: config.acp_prefix.clone(),
            task_semaphore: Arc::new(Semaphore::new(semaphore_permits)),
            session_idle_timeout_secs: config.session_idle_timeout_secs,
            wasm_max_module_size_bytes: config.wasm_max_module_size_bytes,
            wait_for_exit_timeout_secs: config.wait_for_exit_timeout_secs,
        })
    }

    /// Returns (or lazily creates) the sandbox directory for a session.
    fn session_dir(&self, session_id: &str) -> PathBuf {
        let mut sessions = self.sessions.borrow_mut();
        if let Some(s) = sessions.get_mut(session_id) {
            s.last_activity = std::time::Instant::now();
            return s.dir.clone();
        }
        let dir = self.session_root.join(session_id);
        sessions.insert(session_id.to_string(), WasmSession::new(dir.clone()));
        dir
    }

    /// Updates the last_activity timestamp for a session (called on every access).
    pub fn tick_last_activity(&self, session_id: &str) {
        if let Some(s) = self.sessions.borrow_mut().get_mut(session_id) {
            s.last_activity = std::time::Instant::now();
        }
    }

    /// Cleans up sessions that have been idle longer than `session_idle_timeout_secs`.
    pub fn cleanup_idle_sessions(&self) {
        if self.session_idle_timeout_secs == 0 {
            return;
        }
        let now = std::time::Instant::now();
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
                    if let Some(c) = t.output_collector.take() {
                        c.abort();
                    }
                }
            }
            if let Some(session) = self.sessions.borrow_mut().remove(&session_id) {
                let dir = session.dir.clone();
                // Spawn the fs removal as a background task if we're in an async context.
                tokio::task::spawn_local(async move {
                    let _ = tokio::fs::remove_dir_all(&dir).await;
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

    /// Ensures the sandbox directory for a session exists on disk.
    async fn ensure_session_dir(&self, session_id: &str) -> std::io::Result<PathBuf> {
        let dir = self.session_dir(session_id);
        tokio::fs::create_dir_all(&dir).await?;
        Ok(dir)
    }

    // ── Module cache ───────────────────────────────────────────────────────

    /// Derives an on-disk cache key from the absolute path of a `.wasm` file.
    /// Uses `DefaultHasher` for uniqueness (not cryptographic security).
    fn cache_key(path: &std::path::Path) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Returns a compiled `Module` for `wasm_path`, using in-memory and on-disk
    /// caches. Invalidates both caches when the source file's mtime changes.
    fn get_or_compile_module(&self, wasm_path: &std::path::Path) -> Result<Module, anyhow::Error> {
        let abs_path = wasm_path.canonicalize().unwrap_or_else(|_| wasm_path.to_path_buf());

        // Check file size limit before doing anything else.
        let meta = std::fs::metadata(&abs_path)?;
        if self.wasm_max_module_size_bytes > 0
            && meta.len() as usize > self.wasm_max_module_size_bytes
        {
            return Err(anyhow::anyhow!(
                "WASM module too large: {} bytes (limit: {})",
                meta.len(),
                self.wasm_max_module_size_bytes
            ));
        }

        // Get current mtime of source file.
        let current_mtime = meta.modified()?;

        // Check in-memory cache.
        {
            let cached = self.modules.borrow();
            if let Some((m, mtime)) = cached.get(&abs_path) {
                if *mtime == current_mtime {
                    METRICS.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Ok(m.clone());
                }
            }
        }

        // Check on-disk cache.
        if let Some(ref cache_dir) = self.module_cache_dir {
            let key = Self::cache_key(&abs_path);
            let cwasm_path = cache_dir.join(format!("{key}.cwasm"));
            let mtime_path = cache_dir.join(format!("{key}.mtime"));
            let version_path = cache_dir.join(format!("{key}.version"));

            if cwasm_path.exists() && mtime_path.exists() {
                // Check version fingerprint first — skip deserialization if version mismatch.
                let stored_version = std::fs::read_to_string(&version_path).unwrap_or_default();
                let version_ok = stored_version.trim() == env!("CARGO_PKG_VERSION");

                if version_ok {
                    if let Ok(mtime_str) = std::fs::read_to_string(&mtime_path) {
                        let stored_nanos: u64 = mtime_str.trim().parse().unwrap_or(0);
                        let current_nanos = current_mtime
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos() as u64)
                            .unwrap_or(0);
                        if stored_nanos != 0 && stored_nanos == current_nanos {
                            // SAFETY: the engine config is always identical for cached modules
                            // in this runtime — same wasmtime::Config, same Cranelift settings.
                            let cached_module = unsafe { Module::deserialize_file(&self.engine, &cwasm_path) };
                            match cached_module {
                                Ok(m) => {
                                    METRICS.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    self.modules.borrow_mut().insert(abs_path.clone(), (m.clone(), current_mtime));
                                    return Ok(m);
                                }
                                Err(e) => {
                                    tracing::warn!(path = %cwasm_path.display(), error = %e, "on-disk module cache invalid, recompiling");
                                    // Fall through to recompilation below.
                                }
                            }
                        }
                    }
                } else {
                    tracing::warn!(path = %cwasm_path.display(), stored = %stored_version.trim(), current = %env!("CARGO_PKG_VERSION"), "module cache version mismatch, recompiling");
                }
            }
        }

        // Compile fresh from source (cache miss).
        METRICS.cache_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let m = Module::from_file(&self.engine, &abs_path)?;

        // Write on-disk cache if configured.
        if let Some(ref cache_dir) = self.module_cache_dir {
            let key = Self::cache_key(&abs_path);
            // Ensure the cache directory exists.
            if std::fs::create_dir_all(cache_dir).is_ok() {
                if let Ok(serialized) = m.serialize() {
                    let cwasm_path = cache_dir.join(format!("{key}.cwasm"));
                    let mtime_path = cache_dir.join(format!("{key}.mtime"));
                    let version_path = cache_dir.join(format!("{key}.version"));
                    let current_nanos = current_mtime
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);
                    let _ = std::fs::write(&cwasm_path, &serialized);
                    let _ = std::fs::write(&mtime_path, current_nanos.to_string());
                    let _ = std::fs::write(&version_path, env!("CARGO_PKG_VERSION"));
                }
            }
        }

        self.modules.borrow_mut().insert(abs_path, (m.clone(), current_mtime));
        Ok(m)
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
        tokio::fs::create_dir_all(&cwd)
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
        METRICS.native_tasks_started.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let output_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(&cwd)
            .env_clear()
            .env("HOME", &cwd)
            .env("TMPDIR", sandbox_dir.join("tmp"))
            .env("PWD", &cwd);

        // Forward safe env vars from request.
        for env_var in &req.env {
            cmd.env(&env_var.name, &env_var.value);
        }

        // Capture stdout + stderr separately, merge into output_buf.
        // Pipe stdin so callers can write to the process via handle_write_to_terminal.
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            agent_client_protocol::Error::new(-32603, format!("Failed to spawn process: {e}"))
        })?;

        let stdin = child.stdin.take();

        let terminal_id = Uuid::new_v4().to_string();

        // Spawn background task to collect stdout.
        let buf_stdout = Arc::clone(&output_buf);
        let limit = self.output_byte_limit;
        let stdout_handle = if let Some(mut stdout) = child.stdout.take() {
            tokio::task::spawn(async move {
                let mut chunk = [0u8; 4096];
                loop {
                    match stdout.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => WasmTerminal::append_output(&buf_stdout, limit, &chunk[..n]),
                        Err(_) => break,
                    }
                }
            })
        } else {
            tokio::task::spawn(async {})
        };

        // Spawn background task to collect stderr.
        let buf_stderr = Arc::clone(&output_buf);
        let stderr_handle = if let Some(mut stderr) = child.stderr.take() {
            tokio::task::spawn(async move {
                let mut chunk = [0u8; 4096];
                loop {
                    match stderr.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => WasmTerminal::append_output(&buf_stderr, limit, &chunk[..n]),
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
            child: Some(child),
            stdin,
            output_buf,
            output_byte_limit: self.output_byte_limit,
            output_collector: Some(collector),
            exit_status: None,
            wasm_exit_status: None,
        };

        info!(session_id, terminal_id, command, "Terminal created");
        self.terminals
            .borrow_mut()
            .insert(terminal_id.clone(), terminal);

        // Feature 3: track terminal → session mapping.
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
    /// Feature 1: Uses the compiled module cache to avoid re-compilation.
    /// Feature 2: Applies `wasm_timeout_secs` if configured.
    /// Feature 4: `create_terminal` returns immediately; WASM runs in background.
    async fn run_wasm_terminal(
        &self,
        session_id: &str,
        command: &str,
        args: &[String],
        env_vars: &[agent_client_protocol::EnvVariable],
        _cwd: &std::path::Path,
        sandbox_dir: &std::path::Path,
    ) -> agent_client_protocol::Result<CreateTerminalResponse> {
        let wasm_path = PathBuf::from(command);
        let env_pairs: Vec<(String, String)> = env_vars
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect();

        // Feature 1: Get or compile the module from the cache (with mtime invalidation).
        let module = self
            .get_or_compile_module(&wasm_path)
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        // Feature 4: Shared output buffer and exit status for background task.
        let output_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let wasm_exit: Arc<Mutex<Option<TerminalExitStatus>>> = Arc::new(Mutex::new(None));

        let terminal_id = Uuid::new_v4().to_string();

        // Clone what the background task needs.
        let engine = self.engine.clone();
        let buf = Arc::clone(&output_buf);
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
        let semaphore = Arc::clone(&self.task_semaphore);
        // Build full argv: command (wasm_path) as argv[0], then user-supplied args.
        let mut argv_cloned: Vec<String> = Vec::with_capacity(args.len() + 1);
        argv_cloned.push(command.to_string());
        argv_cloned.extend_from_slice(args);
        let env_pairs_cloned = env_pairs.clone();
        let sandbox_dir_cloned = sandbox_dir.to_path_buf();
        let session_id_cloned = session_id.to_string();

        METRICS.wasm_tasks_started.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Feature 4: Spawn WASM execution as a background local task.
        let collector = tokio::task::spawn_local(async move {
            // Backpressure: acquire a permit before executing. The permit is held
            // for the lifetime of the task and released when dropped.
            let _permit = semaphore.acquire_owned().await.expect("semaphore closed unexpectedly");
            let result = wasm::run_module_compiled(
                &engine,
                module,
                &argv_cloned,
                &env_pairs_cloned,
                &sandbox_dir_cloned,
                Arc::clone(&buf),
                limit,
                timeout,
                memory_limit,
                nats_for_task,
                session_id_cloned,
                allow_network,
                auto_allow,
                fuel_limit,
                host_call_limit,
                acp_prefix,
            )
            .await;
            let exit_status = match result {
                Ok(status) => status,
                Err(e) => TerminalExitStatus::new().signal(Some(format!("{e}"))),
            };
            // Update metrics based on how the task exited.
            if exit_status.signal.is_some() {
                METRICS.wasm_tasks_faulted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if exit_status.signal.as_deref() == Some("fuel_exhausted") {
                    METRICS.wasm_fuel_exhausted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                METRICS.wasm_tasks_completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            *exit_arc.lock().unwrap() = Some(exit_status);
        });

        let terminal = WasmTerminal {
            child: None,
            stdin: None,
            output_buf,
            output_byte_limit: self.output_byte_limit,
            output_collector: Some(collector),
            exit_status: None,
            wasm_exit_status: Some(wasm_exit),
        };

        info!(session_id, terminal_id, command, "WASM module executing (background)");
        self.terminals
            .borrow_mut()
            .insert(terminal_id.clone(), terminal);

        // Feature 3: track terminal → session mapping.
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
        let mut terminals = self.terminals.borrow_mut();
        match terminals.get_mut(&terminal_id) {
            Some(t) => {
                t.kill().await;
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
        let mut terminals = self.terminals.borrow_mut();
        match terminals.get_mut(terminal_id) {
            Some(t) => {
                if t.write_stdin(data).await {
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
    pub fn handle_close_terminal_stdin(&self, terminal_id: &str) {
        let mut terminals = self.terminals.borrow_mut();
        if let Some(t) = terminals.get_mut(terminal_id) {
            t.close_stdin();
        }
    }

    pub async fn handle_release_terminal(
        &self,
        req: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        let terminal_id = req.terminal_id.0.as_ref().to_string();

        // Feature 3: Look up the session_id before removing.
        let maybe_session_id = self
            .terminal_session
            .borrow()
            .get(&terminal_id)
            .cloned();

        let mut terminals = self.terminals.borrow_mut();
        match terminals.remove(&terminal_id) {
            Some(mut t) => {
                t.kill().await;
                if let Some(collector) = t.output_collector.take() {
                    collector.abort();
                }
                debug!(terminal_id, "Terminal released");

                // Feature 3: Clean up tracking maps and trigger session cleanup if needed.
                drop(terminals); // Release borrow before potentially cleaning up session.
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
        // Take the terminal out temporarily to call `.wait()` (needs &mut).
        let mut terminal = {
            let mut terminals = self.terminals.borrow_mut();
            match terminals.remove(&terminal_id) {
                Some(t) => t,
                None => {
                    return Err(agent_client_protocol::Error::new(
                        -32602,
                        format!("Unknown terminal: {terminal_id}"),
                    ));
                }
            }
        };

        let timeout = self.wait_for_exit_timeout_secs;
        let status = if timeout == 0 {
            terminal.wait().await
        } else {
            match tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                terminal.wait(),
            )
            .await
            {
                Ok(s) => s,
                Err(_) => {
                    tracing::warn!(terminal_id, "Terminal wait timed out after {timeout}s — killing");
                    terminal.kill().await;
                    TerminalExitStatus::new().signal(Some("wait_timeout".to_string()))
                }
            }
        };

        // Put it back so `terminal_output` can still be called after exit.
        self.terminals.borrow_mut().insert(terminal_id, terminal);
        Ok(WaitForTerminalExitResponse::new(status))
    }

    /// Feature 3: Clean up all state for a session after all its terminals are released.
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
            if let Some(mut t) = self.terminals.borrow_mut().remove(&tid) {
                t.kill().await;
                if let Some(c) = t.output_collector.take() {
                    c.abort();
                }
            }
        }

        // Delete the sandbox directory and free session state.
        if let Some(session) = self.sessions.borrow_mut().remove(session_id) {
            let _ = tokio::fs::remove_dir_all(&session.dir).await;
            debug!(session_id, "Session sandbox cleaned up");
        }
    }

    // ── Filesystem operations ──────────────────────────────────────────────

    pub async fn handle_write_text_file(
        &self,
        session_id: &str,
        req: WriteTextFileRequest,
    ) -> agent_client_protocol::Result<WriteTextFileResponse> {
        let sandbox_dir = self
            .ensure_session_dir(session_id)
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
                format!(
                    "Path {} is outside the session sandbox",
                    req.path.display()
                ),
            )
        })?;

        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;
        }

        tokio::fs::write(&dest, req.content.as_bytes())
            .await
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        debug!(
            session_id,
            path = %dest.display(),
            bytes = req.content.len(),
            "File written"
        );
        let _ = sandbox_dir;
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
                format!(
                    "Path {} is outside the session sandbox",
                    req.path.display()
                ),
            )
        })?;

        let content = tokio::fs::read_to_string(&src)
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
                let outcome = RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(first.option_id.clone()),
                );
                return Ok(RequestPermissionResponse::new(outcome));
            }
        }
        // No options or permissions disabled → cancel.
        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ))
    }

    // ── Session notification ───────────────────────────────────────────────

    pub fn handle_session_notification(&self, notif: SessionNotification) {
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
