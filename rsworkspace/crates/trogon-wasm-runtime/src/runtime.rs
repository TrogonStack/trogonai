use crate::config::Config;

/// Cache fingerprint combining crate version and wasmtime version.
/// The wasmtime version is injected at build time from Cargo.lock by build.rs,
/// so it updates automatically when the dependency is bumped.
const CACHE_FINGERPRINT: &str =
    concat!(env!("CARGO_PKG_VERSION"), "+wasmtime-", env!("WASMTIME_VERSION"));
use crate::metrics::METRICS;
use crate::session::WasmSession;
use crate::terminal::{TerminalKind, WasmTerminal};
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

    /// Returns (and lazily registers) the sandbox path for a session.
    ///
    /// Does **not** create the directory on disk — call [`ensure_session_dir`] for that.
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

    /// Removes any leftover session sandbox directories from a previous run.
    /// Called once at startup before accepting any requests.
    pub async fn cleanup_stale_sessions(&self) {
        let root = &self.session_root;
        let mut rd = match tokio::fs::read_dir(root).await {
            Ok(r) => r,
            Err(_) => return, // root doesn't exist yet, nothing to clean
        };
        let mut cleaned = 0u32;
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                if tokio::fs::remove_dir_all(&path).await.is_ok() {
                    cleaned += 1;
                }
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
        tokio::fs::create_dir_all(&dir).await?;
        // Create the tmp sub-directory so that TMPDIR is usable for native processes.
        tokio::fs::create_dir_all(dir.join("tmp")).await?;
        Ok(dir)
    }

    // ── Module cache ───────────────────────────────────────────────────────

    /// Derives an on-disk cache key from the absolute path of a `.wasm` file.
    ///
    /// Uses FNV-1a 64-bit, which is deterministic across Rust versions and
    /// process restarts. `DefaultHasher` is explicitly avoided because its
    /// implementation is "subject to change" between compiler versions, which
    /// would silently invalidate on-disk caches after a toolchain upgrade.
    fn cache_key(path: &std::path::Path) -> String {
        let mut hash: u64 = 0xcbf29ce484222325;
        for &byte in path.as_os_str().as_encoded_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
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
                let version_ok = stored_version.trim() == CACHE_FINGERPRINT;

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
                    tracing::warn!(path = %cwasm_path.display(), stored = %stored_version.trim(), current = %CACHE_FINGERPRINT, "module cache version mismatch, recompiling");
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
                    let _ = std::fs::write(&version_path, CACHE_FINGERPRINT);
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
            kind: TerminalKind::Native { child: Some(child), stdin },
            output_buf,
            output_byte_limit: self.output_byte_limit,
            output_collector: Some(collector),
            exit_status: None,
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
        let cwd_cloned = cwd.to_path_buf();
        let session_id_cloned = session_id.to_string();

        // Feature 4: Spawn WASM execution as a background local task.
        let collector = tokio::task::spawn_local(async move {
            // Backpressure: acquire a permit before executing. The permit is held
            // for the lifetime of the task and released when dropped.
            let _permit = semaphore.acquire_owned().await.expect("semaphore closed unexpectedly");
            // Count tasks that actually start executing, not just those queued for a permit.
            METRICS.wasm_tasks_started.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let result = wasm::run_module_compiled(
                &engine,
                wasm::WasmExecConfig {
                    module,
                    argv: argv_cloned,
                    env_vars: env_pairs_cloned,
                    sandbox_dir: sandbox_dir_cloned.clone(),
                    cwd: if cwd_cloned != sandbox_dir_cloned {
                        Some(cwd_cloned)
                    } else {
                        None
                    },
                    output_buf: Arc::clone(&buf),
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
                },
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
            kind: TerminalKind::Wasm { exit_arc: wasm_exit },
            output_buf,
            output_byte_limit: self.output_byte_limit,
            output_collector: Some(collector),
            exit_status: None,
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
        self.tick_last_activity_for_terminal(terminal_id);
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
                        TerminalKind::Wasm { exit_arc } => (None, Some(std::sync::Arc::clone(exit_arc))),
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
                        tracing::warn!(terminal_id, "Terminal wait timed out after {timeout}s — killing");
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
                .and_then(|arc| arc.lock().ok())
                .and_then(|g| g.clone())
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
                    if let Some(status) = arc.lock().ok().and_then(|g| g.clone()) {
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

        // Write atomically: write to a temp file first, then rename.
        let tmp_dest = dest.with_extension("tmp");
        tokio::fs::write(&tmp_dest, req.content.as_bytes())
            .await
            .map_err(|e| agent_client_protocol::Error::new(-32603, format!("Failed to write file: {e}")))?;
        tokio::fs::rename(&tmp_dest, &dest)
            .await
            .map_err(|e| {
                // Clean up temp file on failure (best-effort).
                let _ = std::fs::remove_file(&tmp_dest);
                agent_client_protocol::Error::new(-32603, format!("Failed to finalize file write: {e}"))
            })?;

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
