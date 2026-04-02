use crate::config::Config;
use crate::session::WasmSession;
use crate::terminal::WasmTerminal;
use crate::wasm;
use agent_client_protocol::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, TerminalId,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, info};
use uuid::Uuid;
use wasmtime::Engine;

/// Central execution runtime — one instance per process, shared across sessions.
///
/// Manages per-session sandbox directories and terminal processes.
/// The wasmtime `Engine` is pre-initialized for WASM module execution;
/// actual `.wasm` dispatch is added in the next step (see `run_wasm`).
pub struct WasmRuntime {
    /// Shared wasmtime engine (Cranelift JIT, reuses compiled module cache).
    /// Scaffolded for WASM module execution; `.wasm` dispatch is the next step.
    #[allow(dead_code)]
    pub engine: Engine,
    /// Root directory under which per-session sandboxes are created.
    session_root: PathBuf,
    /// Maximum output bytes retained per terminal.
    output_byte_limit: usize,
    /// When `true`, the first permission option is auto-selected.
    auto_allow_permissions: bool,
    /// Live sessions keyed by ACP session_id.
    sessions: RefCell<HashMap<String, WasmSession>>,
    /// All terminals across all sessions, keyed by terminal_id.
    /// Indexed globally so `terminal_output` / `kill` don't need session_id.
    terminals: RefCell<HashMap<String, WasmTerminal>>,
}

impl WasmRuntime {
    pub fn new(config: &Config) -> Result<Self, wasmtime::Error> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.async_support(true);
        wasm_config.consume_fuel(true);
        let engine = Engine::new(&wasm_config)?;
        Ok(Self {
            engine,
            session_root: config.session_root.clone(),
            output_byte_limit: config.output_byte_limit,
            auto_allow_permissions: config.auto_allow_permissions,
            sessions: RefCell::new(HashMap::new()),
            terminals: RefCell::new(HashMap::new()),
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
        };

        info!(session_id, terminal_id, command, "Terminal created");
        self.terminals
            .borrow_mut()
            .insert(terminal_id.clone(), terminal);

        Ok(CreateTerminalResponse::new(TerminalId::new(
            terminal_id.as_str(),
        )))
    }

    /// Runs a `.wasm` module synchronously using wasmtime + WASIp1.
    ///
    /// The module's stdout+stderr are buffered. A synthetic `WasmTerminal`
    /// is registered so callers can retrieve output via `terminal_output` and
    /// `wait_for_terminal_exit` using the same API as native terminals.
    async fn run_wasm_terminal(
        &self,
        session_id: &str,
        command: &str,
        args: &[String],
        env_vars: &[agent_client_protocol::EnvVariable],
        _cwd: &std::path::Path,
        sandbox_dir: &std::path::Path,
    ) -> agent_client_protocol::Result<CreateTerminalResponse> {
        let wasm_path = std::path::Path::new(command);
        let env_pairs: Vec<(String, String)> = env_vars
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect();

        let output: crate::wasm::WasmOutput = wasm::run_module(
            &self.engine,
            wasm_path,
            args,
            &env_pairs,
            sandbox_dir,
            self.output_byte_limit,
        )
        .await
        .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

        // Merge stdout + stderr into the output buffer (stdout first).
        let combined = [output.stdout.as_slice(), output.stderr.as_slice()].concat();
        let output_buf = std::sync::Arc::new(std::sync::Mutex::new(combined));

        let terminal_id = Uuid::new_v4().to_string();
        let terminal = WasmTerminal {
            child: None, // already exited
            output_buf,
            output_byte_limit: self.output_byte_limit,
            output_collector: None,
            exit_status: Some(output.exit_status),
        };

        info!(session_id, terminal_id, command, "WASM module executed");
        self.terminals
            .borrow_mut()
            .insert(terminal_id.clone(), terminal);

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
        let mut terminals = self.terminals.borrow_mut();
        match terminals.remove(&terminal_id) {
            Some(mut t) => {
                t.kill().await;
                if let Some(collector) = t.output_collector.take() {
                    collector.abort();
                }
                debug!(terminal_id, "Terminal released");
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
