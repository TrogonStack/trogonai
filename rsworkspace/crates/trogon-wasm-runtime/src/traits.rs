use agent_client_protocol::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use std::io;
use std::path::{Path, PathBuf};

// ── Runtime trait ─────────────────────────────────────────────────────────────

/// Abstracts all ACP handler methods implemented by `WasmRuntime`.
///
/// Making the dispatcher generic over this trait enables unit-testing routing
/// logic with a `MockRuntime` without a real WASM engine or NATS server.
pub trait Runtime {
    fn handle_create_terminal(
        &self,
        session_id: &str,
        req: CreateTerminalRequest,
    ) -> impl std::future::Future<Output = agent_client_protocol::Result<CreateTerminalResponse>>;

    fn handle_terminal_output(
        &self,
        req: TerminalOutputRequest,
    ) -> impl std::future::Future<Output = agent_client_protocol::Result<TerminalOutputResponse>>;

    fn handle_kill_terminal(
        &self,
        req: KillTerminalRequest,
    ) -> impl std::future::Future<Output = agent_client_protocol::Result<KillTerminalResponse>>;

    fn handle_write_to_terminal(
        &self,
        terminal_id: &str,
        data: &[u8],
    ) -> impl std::future::Future<Output = agent_client_protocol::Result<()>>;

    fn handle_close_terminal_stdin(&self, terminal_id: &str) -> agent_client_protocol::Result<()>;

    fn handle_release_terminal(
        &self,
        req: ReleaseTerminalRequest,
    ) -> impl std::future::Future<Output = agent_client_protocol::Result<ReleaseTerminalResponse>>;

    fn handle_wait_for_terminal_exit(
        &self,
        req: WaitForTerminalExitRequest,
    ) -> impl std::future::Future<Output = agent_client_protocol::Result<WaitForTerminalExitResponse>>;

    fn handle_write_text_file(
        &self,
        session_id: &str,
        req: WriteTextFileRequest,
    ) -> impl std::future::Future<Output = agent_client_protocol::Result<WriteTextFileResponse>>;

    fn handle_read_text_file(
        &self,
        session_id: &str,
        req: ReadTextFileRequest,
    ) -> impl std::future::Future<Output = agent_client_protocol::Result<ReadTextFileResponse>>;

    fn handle_request_permission(
        &self,
        req: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse>;

    fn handle_session_notification(&self, notif: SessionNotification);

    fn list_sessions(&self) -> Vec<String>;

    fn list_terminals(&self) -> Vec<(String, String)>;

    /// Synchronously initiates cleanup of idle sessions.
    ///
    /// Implementations that need async work (e.g. `tokio::task::spawn_local`)
    /// must only call this from within a `LocalSet` context.
    fn cleanup_idle_sessions(&self);

    fn cleanup_all_sessions(&self) -> impl std::future::Future<Output = ()>;
}

// ── WasmExecutor trait ────────────────────────────────────────────────────────

/// All parameters needed to run a WASM module as a terminal.
///
/// Passed by value to [`WasmExecutor::run`] so each invocation owns its config.
pub struct WasmRunConfig<N: NatsBroker + Send + Sync> {
    /// Absolute path to the `.wasm` module file.
    pub wasm_path: std::path::PathBuf,
    /// Full argv, including argv[0] (the module path / program name).
    pub argv: Vec<String>,
    /// Environment variables to expose inside the module.
    pub env_vars: Vec<(String, String)>,
    /// Host directory preopened as `/` (the session sandbox root).
    pub sandbox_dir: std::path::PathBuf,
    /// Optional working directory preopened as `.` when different from `sandbox_dir`.
    pub cwd: Option<std::path::PathBuf>,
    /// Shared buffer where stdout/stderr are appended.
    pub output_buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    /// Set to `true` by I/O reader tasks when bytes are dropped due to the limit.
    pub was_truncated: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Maximum bytes kept in `output_buf`.
    pub output_byte_limit: usize,
    /// Optional wall-clock execution timeout.
    pub timeout_secs: Option<u64>,
    /// Optional linear-memory limit in bytes.
    pub memory_limit_bytes: Option<usize>,
    /// Optional NATS client for trogon.* host functions.
    pub nats_client: Option<N>,
    /// Session identifier forwarded to host functions for logging / NATS subjects.
    pub session_id: String,
    /// Allow WASI network access (`inherit_network`).
    pub allow_network: bool,
    /// Auto-select the first permission option instead of asking over NATS.
    pub auto_allow_permissions: bool,
    /// Wasmtime fuel budget (instruction limit). 0 means u64::MAX.
    pub fuel_limit: u64,
    /// Maximum number of trogon.* host calls. Returns -1 when exhausted.
    pub host_call_limit: u32,
    /// ACP subject prefix for NATS permission requests.
    pub acp_prefix: String,
}

/// Abstracts WASM module compilation and execution.
///
/// Implemented by [`crate::wasm::RealWasmExecutor`] for production and by
/// `MockWasmExecutor` in tests, enabling unit tests of the WASM terminal path
/// without running real WASM modules.
pub trait WasmExecutor<N: NatsBroker + Send + Sync>: Clone + 'static {
    /// Optional eager pre-flight check run before spawning the background task.
    ///
    /// Called synchronously (via `spawn_blocking`) before `run` to surface
    /// errors like "module not found" or "module too large" at `create_terminal`
    /// time. The default implementation is a no-op (returns `Ok(())`).
    fn validate(&self, _wasm_path: &std::path::Path) -> Result<(), anyhow::Error> {
        Ok(())
    }

    fn run(
        &self,
        config: WasmRunConfig<N>,
    ) -> impl std::future::Future<
        Output = Result<agent_client_protocol::TerminalExitStatus, anyhow::Error>,
    >;
}

// ── NatsBroker trait ──────────────────────────────────────────────────────────

/// Abstracts the NATS operations used by the dispatcher and WASM host functions.
///
/// Implemented for `async_nats::Client` in production and for `MockBroker`
/// in tests, enabling pure unit tests of routing logic without a real NATS
/// server.
///
/// `Send + Sync` are required because WASM host functions registered via
/// `func_new_async` must produce `Send` futures, and those futures capture
/// `Caller<'_, WasmStoreData<N>>` which is `Send` only when `N: Send`.
pub trait NatsBroker: Clone + Send + Sync + 'static {
    /// The type of subscription stream returned by `subscribe`.
    ///
    /// Must be `Unpin` so it can be used inside `tokio::select!`.
    /// Must be `Send` because it is stored in `WasmStoreData` which is
    /// accessed from `Send` futures inside wasmtime host functions.
    type Sub: futures::Stream<Item = async_nats::Message> + Unpin + Send + 'static;

    fn subscribe(
        &self,
        subject: &str,
    ) -> impl std::future::Future<Output = Result<Self::Sub, Box<dyn std::error::Error + Send + Sync>>>
           + Send;

    fn publish(
        &self,
        subject: async_nats::Subject,
        payload: bytes::Bytes,
    ) -> impl std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send;

    /// Sends a request and awaits one reply.
    fn request(
        &self,
        subject: impl Into<String> + Send,
        payload: bytes::Bytes,
    ) -> impl std::future::Future<
        Output = Result<async_nats::Message, Box<dyn std::error::Error + Send + Sync>>,
    > + Send;
}

/// Blanket implementation of `NatsBroker` for the real `async_nats::Client`.
impl NatsBroker for async_nats::Client {
    type Sub = async_nats::Subscriber;

    async fn subscribe(
        &self,
        subject: &str,
    ) -> Result<Self::Sub, Box<dyn std::error::Error + Send + Sync>> {
        self.subscribe(subject.to_string())
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn publish(
        &self,
        subject: async_nats::Subject,
        payload: bytes::Bytes,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        async_nats::Client::publish(self, subject, payload)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn request(
        &self,
        subject: impl Into<String> + Send,
        payload: bytes::Bytes,
    ) -> Result<async_nats::Message, Box<dyn std::error::Error + Send + Sync>> {
        async_nats::Client::request(self, subject.into(), payload)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
}

// ── Fs trait ─────────────────────────────────────────────────────────────────

/// Abstracts async filesystem operations used by `WasmRuntime` for session
/// sandbox management and file I/O.
///
/// Implemented by `TokioFs` for production and `MockFs` for tests, enabling
/// pure unit tests of file-handling logic without touching the real filesystem.
pub trait Fs: Clone + 'static {
    fn create_dir_all(&self, path: &Path) -> impl std::future::Future<Output = io::Result<()>>;

    fn write(&self, path: &Path, data: &[u8]) -> impl std::future::Future<Output = io::Result<()>>;

    fn rename(&self, from: &Path, to: &Path) -> impl std::future::Future<Output = io::Result<()>>;

    fn read_to_string(&self, path: &Path) -> impl std::future::Future<Output = io::Result<String>>;

    fn remove_dir_all(&self, path: &Path) -> impl std::future::Future<Output = io::Result<()>>;

    fn remove_file(&self, path: &Path) -> impl std::future::Future<Output = io::Result<()>>;

    /// Returns all immediate child directories of `path`.
    fn list_subdirs(
        &self,
        path: &Path,
    ) -> impl std::future::Future<Output = io::Result<Vec<PathBuf>>>;
}

// ── TokioFs ───────────────────────────────────────────────────────────────────

/// Real filesystem implementation backed by `tokio::fs`.
#[derive(Clone, Copy, Debug)]
pub struct TokioFs;

impl Fs for TokioFs {
    async fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }

    async fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        tokio::fs::write(path, data).await
    }

    async fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        tokio::fs::rename(from, to).await
    }

    async fn read_to_string(&self, path: &Path) -> io::Result<String> {
        tokio::fs::read_to_string(path).await
    }

    async fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        tokio::fs::remove_dir_all(path).await
    }

    async fn remove_file(&self, path: &Path) -> io::Result<()> {
        tokio::fs::remove_file(path).await
    }

    async fn list_subdirs(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut rd = tokio::fs::read_dir(path).await?;
        let mut dirs = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            let p = entry.path();
            if p.is_dir() {
                dirs.push(p);
            }
        }
        Ok(dirs)
    }
}

// ── ChildProcessHandle trait ──────────────────────────────────────────────────

/// Abstracts a spawned native process with stdin/stdout/stderr pipes.
///
/// The associated I/O types must be `Send` because stdout and stderr are
/// drained by `tokio::task::spawn` (not `spawn_local`) background tasks.
pub trait ChildProcessHandle: 'static {
    type Stdin: tokio::io::AsyncWrite + Unpin + Send + 'static;
    type Stdout: tokio::io::AsyncRead + Unpin + Send + 'static;
    type Stderr: tokio::io::AsyncRead + Unpin + Send + 'static;

    fn take_stdin(&mut self) -> Option<Self::Stdin>;
    fn take_stdout(&mut self) -> Option<Self::Stdout>;
    fn take_stderr(&mut self) -> Option<Self::Stderr>;

    fn wait(&mut self) -> impl std::future::Future<Output = io::Result<std::process::ExitStatus>>;

    fn kill(&mut self) -> impl std::future::Future<Output = io::Result<()>>;
}

/// Implement `ChildProcessHandle` for the real `tokio::process::Child`.
impl ChildProcessHandle for tokio::process::Child {
    type Stdin = tokio::process::ChildStdin;
    type Stdout = tokio::process::ChildStdout;
    type Stderr = tokio::process::ChildStderr;

    fn take_stdin(&mut self) -> Option<Self::Stdin> {
        self.stdin.take()
    }

    fn take_stdout(&mut self) -> Option<Self::Stdout> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<Self::Stderr> {
        self.stderr.take()
    }

    async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        tokio::process::Child::wait(self).await
    }

    async fn kill(&mut self) -> io::Result<()> {
        tokio::process::Child::kill(self).await
    }
}

// ── ProcessSpawner trait ──────────────────────────────────────────────────────

/// Abstracts spawning of native OS processes.
///
/// Implemented by `TokioProcessSpawner` for production and `MockProcessSpawner`
/// for tests, enabling unit tests of terminal creation logic without real
/// processes.
pub trait ProcessSpawner: Clone + 'static {
    type Handle: ChildProcessHandle;

    fn spawn(
        &self,
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: &Path,
        home: &Path,
        tmpdir: &Path,
    ) -> impl std::future::Future<Output = io::Result<Self::Handle>>;
}

// ── TokioProcessSpawner ───────────────────────────────────────────────────────

/// Real process spawner backed by `tokio::process::Command`.
#[derive(Clone, Copy, Debug)]
pub struct TokioProcessSpawner;

impl ProcessSpawner for TokioProcessSpawner {
    type Handle = tokio::process::Child;

    async fn spawn(
        &self,
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: &Path,
        home: &Path,
        tmpdir: &Path,
    ) -> io::Result<tokio::process::Child> {
        use tokio::process::Command;
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", home)
            .env("TMPDIR", tmpdir)
            .env("PWD", cwd);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.spawn()
    }
}

// ── Clock trait ───────────────────────────────────────────────────────────────

/// Abstracts wall-clock access used for session idle-timeout tracking.
///
/// Implemented by `StdClock` for production and `MockClock` in tests,
/// enabling deterministic idle-timeout tests without real `sleep` calls.
pub trait Clock: Clone + 'static {
    fn now(&self) -> std::time::Instant;
}

/// Real clock backed by `std::time::Instant::now()`.
#[derive(Clone, Copy, Debug)]
pub struct StdClock;

impl Clock for StdClock {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

// ── IdGenerator trait ─────────────────────────────────────────────────────────

/// Abstracts unique ID generation used for terminal IDs and atomic temp-file names.
///
/// Implemented by `UuidGenerator` for production and `MockIdGenerator` in tests,
/// enabling predictable IDs in unit tests.
pub trait IdGenerator: Clone + 'static {
    fn new_id(&self) -> String;
}

/// Real ID generator backed by `uuid::Uuid::new_v4()`.
#[derive(Clone, Copy, Debug)]
pub struct UuidGenerator;

impl IdGenerator for UuidGenerator {
    fn new_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

// ── TaskLimiter trait ─────────────────────────────────────────────────────────

/// Abstracts concurrent-task limiting used to cap simultaneous WASM executions.
///
/// Implemented by `SemaphoreTaskLimiter` for production and `UnlimitedTaskLimiter`
/// in tests, enabling unit tests without a real semaphore.
pub trait TaskLimiter: Clone + 'static {
    /// Token held for the duration of a task; dropped when the task finishes.
    type Permit: 'static;
    fn acquire(&self) -> impl std::future::Future<Output = Self::Permit>;
}

/// Real implementation backed by `tokio::sync::Semaphore`.
#[derive(Clone)]
pub struct SemaphoreTaskLimiter(std::sync::Arc<tokio::sync::Semaphore>);

impl SemaphoreTaskLimiter {
    pub fn new(permits: usize) -> Self {
        Self(std::sync::Arc::new(tokio::sync::Semaphore::new(permits)))
    }
}

impl TaskLimiter for SemaphoreTaskLimiter {
    type Permit = tokio::sync::OwnedSemaphorePermit;
    async fn acquire(&self) -> Self::Permit {
        std::sync::Arc::clone(&self.0)
            .acquire_owned()
            .await
            .expect("task semaphore closed unexpectedly")
    }
}

// ── SyncFs trait ─────────────────────────────────────────────────────────────

/// Metadata returned by [`SyncFs::metadata`].
pub struct FileMetadata {
    pub len: u64,
    pub modified: std::time::SystemTime,
}

/// Abstracts blocking filesystem operations used by `RealWasmExecutor`'s
/// on-disk module cache.
///
/// Implemented by `StdSyncFs` for production and `MockSyncFs` in tests,
/// enabling unit tests without touching the real filesystem.
pub trait SyncFs: Clone + Send + 'static {
    fn metadata(&self, path: &std::path::Path) -> std::io::Result<FileMetadata>;
    fn read_to_string(&self, path: &std::path::Path) -> std::io::Result<String>;
    fn create_dir_all(&self, path: &std::path::Path) -> std::io::Result<()>;
    fn write(&self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()>;
}

/// Real implementation backed by `std::fs`.
#[derive(Clone, Copy, Debug)]
pub struct StdSyncFs;

impl SyncFs for StdSyncFs {
    fn metadata(&self, path: &std::path::Path) -> std::io::Result<FileMetadata> {
        let m = std::fs::metadata(path)?;
        Ok(FileMetadata {
            len: m.len(),
            modified: m.modified()?,
        })
    }

    fn read_to_string(&self, path: &std::path::Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn create_dir_all(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn write(&self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, data)
    }
}
