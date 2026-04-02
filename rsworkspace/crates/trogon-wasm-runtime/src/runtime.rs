use crate::config::Config;
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
        })
    }

    /// Returns (or lazily creates) the sandbox directory for a session.
    fn session_dir(&self, session_id: &str) -> PathBuf {
        let mut sessions = self.sessions.borrow_mut();
        if let Some(s) = sessions.get(session_id) {
            return s.dir.clone();
        }
        let dir = self.session_root.join(session_id);
        sessions.insert(session_id.to_string(), WasmSession::new(dir.clone()));
        dir
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

        // Get current mtime of source file.
        let current_mtime = std::fs::metadata(&abs_path)?.modified()?;

        // Check in-memory cache.
        {
            let cached = self.modules.borrow();
            if let Some((m, mtime)) = cached.get(&abs_path) {
                if *mtime == current_mtime {
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

        // Compile fresh from source.
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
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            agent_client_protocol::Error::new(-32603, format!("Failed to spawn process: {e}"))
        })?;

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
        // Build full argv: command (wasm_path) as argv[0], then user-supplied args.
        let mut argv_cloned: Vec<String> = Vec::with_capacity(args.len() + 1);
        argv_cloned.push(command.to_string());
        argv_cloned.extend_from_slice(args);
        let env_pairs_cloned = env_pairs.clone();
        let sandbox_dir_cloned = sandbox_dir.to_path_buf();
        let session_id_cloned = session_id.to_string();

        // Feature 4: Spawn WASM execution as a background local task.
        let collector = tokio::task::spawn_local(async move {
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
            *exit_arc.lock().unwrap() = Some(exit_status);
        });

        let terminal = WasmTerminal {
            child: None,
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

        let status = terminal.wait().await;
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
