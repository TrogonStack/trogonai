/// Unit tests for `WasmRuntime` using in-memory mocks.
///
/// These tests do **not** touch the real filesystem or spawn real OS processes.
/// They verify session management, file I/O routing, and native terminal
/// creation logic using `MockFs` and `MockProcessSpawner`.
use agent_client_protocol::{
    CreateTerminalRequest, ReadTextFileRequest, SessionId, TerminalExitStatus,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use trogon_wasm_runtime::traits::{
    ChildProcessHandle, Clock, Fs, IdGenerator, NatsBroker, ProcessSpawner, SyncFs, TaskLimiter,
    WasmExecutor, WasmRunConfig,
};
use trogon_wasm_runtime::{Config, FileMetadata, WasmRuntime};

// ── MockNatsBroker ────────────────────────────────────────────────────────────

/// A no-op NATS broker for tests that do not exercise NATS functionality.
#[derive(Clone)]
struct MockNatsBroker;

impl NatsBroker for MockNatsBroker {
    type Sub = futures::stream::Empty<async_nats::Message>;

    async fn subscribe(
        &self,
        _subject: &str,
    ) -> Result<Self::Sub, Box<dyn std::error::Error + Send + Sync>> {
        Ok(futures::stream::empty())
    }

    async fn publish(
        &self,
        _subject: async_nats::Subject,
        _payload: bytes::Bytes,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn request(
        &self,
        _subject: impl Into<String> + Send,
        _payload: bytes::Bytes,
    ) -> Result<async_nats::Message, Box<dyn std::error::Error + Send + Sync>> {
        Err("MockNatsBroker does not implement request".into())
    }
}

// ── MockFs ────────────────────────────────────────────────────────────────────

/// In-memory filesystem for tests.
///
/// Files are stored as `HashMap<PathBuf, Vec<u8>>`.
/// Directories are tracked in a `HashSet<PathBuf>`.
#[derive(Clone)]
pub struct MockFs {
    files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
    dirs: Arc<Mutex<HashSet<PathBuf>>>,
}

impl MockFs {
    pub fn new() -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            dirs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Returns the content of a file if it exists.
    pub fn read(&self, path: &Path) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }

    /// Returns `true` if the given directory was created.
    pub fn dir_exists(&self, path: &Path) -> bool {
        self.dirs.lock().unwrap().contains(path)
    }
}

impl Fs for MockFs {
    async fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        // Insert this path and all parent paths.
        let mut dirs = self.dirs.lock().unwrap();
        let mut p = path.to_path_buf();
        loop {
            dirs.insert(p.clone());
            match p.parent() {
                Some(parent) if parent != p => p = parent.to_path_buf(),
                _ => break,
            }
        }
        Ok(())
    }

    async fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), data.to_vec());
        Ok(())
    }

    async fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let data = self
            .files
            .lock()
            .unwrap()
            .remove(from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "source file not found"))?;
        self.files.lock().unwrap().insert(to.to_path_buf(), data);
        Ok(())
    }

    async fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let files = self.files.lock().unwrap();
        let data = files.get(path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("file not found: {}", path.display()))
        })?;
        String::from_utf8(data.clone())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.files
            .lock()
            .unwrap()
            .retain(|p, _| !p.starts_with(path));
        self.dirs
            .lock()
            .unwrap()
            .retain(|p| !p.starts_with(path));
        Ok(())
    }

    async fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }

    async fn list_subdirs(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let dirs = self.dirs.lock().unwrap();
        let result = dirs
            .iter()
            .filter(|p| p.parent() == Some(path))
            .cloned()
            .collect();
        Ok(result)
    }
}

// ── MockProcessHandle ─────────────────────────────────────────────────────────

/// A fake child process that exits immediately with a configurable code.
///
/// Its stdin is discarded; stdout and stderr return pre-seeded bytes.
#[allow(dead_code)]
pub struct MockProcessHandle {
    stdout_content: Vec<u8>,
    stderr_content: Vec<u8>,
    exit_code: i32,
    stdin: Option<tokio::io::Sink>,
    stdout: Option<std::io::Cursor<Vec<u8>>>,
    stderr: Option<std::io::Cursor<Vec<u8>>>,
}

impl MockProcessHandle {
    fn new(exit_code: i32, stdout_content: Vec<u8>, stderr_content: Vec<u8>) -> Self {
        let stdout = stdout_content.clone();
        let stderr = stderr_content.clone();
        Self {
            stdout_content,
            stderr_content,
            exit_code,
            stdin: Some(tokio::io::sink()),
            stdout: Some(std::io::Cursor::new(stdout)),
            stderr: Some(std::io::Cursor::new(stderr)),
        }
    }
}

impl ChildProcessHandle for MockProcessHandle {
    type Stdin = tokio::io::Sink;
    type Stdout = std::io::Cursor<Vec<u8>>;
    type Stderr = std::io::Cursor<Vec<u8>>;

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
        use std::os::unix::process::ExitStatusExt;
        Ok(ExitStatusExt::from_raw(self.exit_code << 8))
    }

    async fn kill(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ── MockProcessSpawner ────────────────────────────────────────────────────────

/// Records spawn calls and returns a `MockProcessHandle`.
#[derive(Clone)]
pub struct MockProcessSpawner {
    pub spawned: Arc<Mutex<Vec<String>>>,
    exit_code: i32,
    stdout_content: Vec<u8>,
}

impl MockProcessSpawner {
    pub fn new() -> Self {
        Self {
            spawned: Arc::new(Mutex::new(Vec::new())),
            exit_code: 0,
            stdout_content: b"mock output\n".to_vec(),
        }
    }

    pub fn with_output(exit_code: i32, stdout: Vec<u8>) -> Self {
        Self {
            spawned: Arc::new(Mutex::new(Vec::new())),
            exit_code,
            stdout_content: stdout,
        }
    }
}

impl ProcessSpawner for MockProcessSpawner {
    type Handle = MockProcessHandle;

    async fn spawn(
        &self,
        command: &str,
        _args: &[String],
        _env: &[(String, String)],
        _cwd: &Path,
        _home: &Path,
        _tmpdir: &Path,
    ) -> io::Result<Self::Handle> {
        self.spawned
            .lock()
            .unwrap()
            .push(command.to_string());
        Ok(MockProcessHandle::new(
            self.exit_code,
            self.stdout_content.clone(),
            vec![],
        ))
    }
}

// ── MockWasmExecutor ──────────────────────────────────────────────────────────

/// A fake WASM executor that returns configurable output without running real WASM.
#[derive(Clone)]
pub struct MockWasmExecutor {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
}

impl MockWasmExecutor {
    pub fn success() -> Self {
        Self {
            exit_code: 0,
            stdout: b"mock wasm output\n".to_vec(),
        }
    }

    pub fn with_exit(code: i32) -> Self {
        Self {
            exit_code: code,
            stdout: vec![],
        }
    }
}

impl<N: NatsBroker + Send + Sync> WasmExecutor<N> for MockWasmExecutor {
    async fn run(
        &self,
        config: WasmRunConfig<N>,
    ) -> Result<TerminalExitStatus, anyhow::Error> {
        config.output_buf.lock().unwrap().extend_from_slice(&self.stdout);
        Ok(TerminalExitStatus::new().exit_code(Some(self.exit_code as u32)))
    }
}

// ── MockClock ─────────────────────────────────────────────────────────────────

/// A controllable clock for deterministic idle-timeout tests.
///
/// Starts at `Instant::now()` and can be advanced by any duration,
/// making session idle-timeout tests deterministic without real `sleep` calls.
#[derive(Clone)]
pub struct MockClock {
    current: Arc<Mutex<std::time::Instant>>,
}

impl MockClock {
    pub fn new() -> Self {
        Self {
            current: Arc::new(Mutex::new(std::time::Instant::now())),
        }
    }

    /// Advances the mock clock by `duration`.
    pub fn advance(&self, duration: std::time::Duration) {
        *self.current.lock().unwrap() += duration;
    }
}

impl Clock for MockClock {
    fn now(&self) -> std::time::Instant {
        *self.current.lock().unwrap()
    }
}

// ── MockIdGenerator ───────────────────────────────────────────────────────────

/// A predictable ID generator for tests.
///
/// Returns `"mock-id-1"`, `"mock-id-2"`, … in sequence.
#[derive(Clone)]
pub struct MockIdGenerator {
    counter: Arc<Mutex<u64>>,
}

impl MockIdGenerator {
    pub fn new() -> Self {
        Self {
            counter: Arc::new(Mutex::new(0)),
        }
    }
}

impl IdGenerator for MockIdGenerator {
    fn new_id(&self) -> String {
        let mut c = self.counter.lock().unwrap();
        *c += 1;
        format!("mock-id-{}", c)
    }
}

// ── MockSyncFs ────────────────────────────────────────────────────────────────

/// In-memory sync filesystem for tests (used by RealWasmExecutor internals).
#[derive(Clone)]
pub struct MockSyncFs {
    files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
}

impl MockSyncFs {
    pub fn new() -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl SyncFs for MockSyncFs {
    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        let files = self.files.lock().unwrap();
        if files.contains_key(path) {
            Ok(FileMetadata {
                len: files[path].len() as u64,
                modified: std::time::SystemTime::UNIX_EPOCH,
            })
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
        }
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let files = self.files.lock().unwrap();
        files
            .get(path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not found"))
            .and_then(|b| {
                String::from_utf8(b.clone())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            })
    }

    fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }

    fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), data.to_vec());
        Ok(())
    }
}

// ── UnlimitedTaskLimiter ──────────────────────────────────────────────────────

/// A no-op task limiter that never blocks (unlimited concurrency).
#[derive(Clone)]
pub struct UnlimitedTaskLimiter;

impl TaskLimiter for UnlimitedTaskLimiter {
    type Permit = ();
    async fn acquire(&self) -> () {}
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn mock_config(session_root: PathBuf) -> Config {
    Config {
        session_root,
        output_byte_limit: 1024 * 1024,
        auto_allow_permissions: true,
        wasm_timeout_secs: None,
        wasm_only: false,
        wasm_memory_limit_bytes: None,
        module_cache_dir: None,
        wasm_allow_network: false,
        wasm_fuel_limit: 1_000_000_000u64,
        wasm_host_call_limit: 10_000u32,
        acp_prefix: "acp".to_string(),
        wasm_max_concurrent_tasks: 32,
        session_idle_timeout_secs: 3600,
        wasm_max_module_size_bytes: 100 * 1024 * 1024,
        wait_for_exit_timeout_secs: 300,
    }
}

const SESSION: &str = "unit-session-01";

// ── fs tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn write_and_read_file_uses_mock_fs() {
    let fs = MockFs::new();
    let runtime = WasmRuntime::with_services(
        &mock_config(PathBuf::from("/sessions")),
        None::<MockNatsBroker>,
        fs.clone(),
        MockProcessSpawner::new(),
        MockWasmExecutor::success(),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    let write_req = WriteTextFileRequest::new(
        SessionId::from(SESSION),
        PathBuf::from("/hello.txt"),
        "Hello, mock!",
    );
    runtime
        .handle_write_text_file(SESSION, write_req)
        .await
        .expect("write should succeed");

    // The file should be in MockFs, not the real disk.
    let dest = PathBuf::from("/sessions").join(SESSION).join("hello.txt");
    let content = fs.read(&dest).expect("file should be in MockFs");
    assert_eq!(content, b"Hello, mock!");

    // Also verify reading back through the runtime works.
    let read_req =
        ReadTextFileRequest::new(SessionId::from(SESSION), PathBuf::from("/hello.txt"));
    let resp = runtime
        .handle_read_text_file(SESSION, read_req)
        .await
        .expect("read should succeed");
    assert_eq!(resp.content, "Hello, mock!");
}

#[tokio::test]
async fn ensure_session_dir_creates_in_mock_fs() {
    let fs = MockFs::new();
    let root = PathBuf::from("/sessions");
    let runtime = WasmRuntime::with_services(
        &mock_config(root.clone()),
        None::<MockNatsBroker>,
        fs.clone(),
        MockProcessSpawner::new(),
        MockWasmExecutor::success(),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    // Trigger session dir creation via a write.
    let req = WriteTextFileRequest::new(
        SessionId::from(SESSION),
        PathBuf::from("/any.txt"),
        "data",
    );
    runtime
        .handle_write_text_file(SESSION, req)
        .await
        .unwrap();

    let session_dir = root.join(SESSION);
    assert!(
        fs.dir_exists(&session_dir),
        "session dir should exist in MockFs"
    );
}

#[tokio::test]
async fn read_missing_file_returns_error() {
    let runtime = WasmRuntime::with_services(
        &mock_config(PathBuf::from("/sessions")),
        None::<MockNatsBroker>,
        MockFs::new(),
        MockProcessSpawner::new(),
        MockWasmExecutor::success(),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    let req = ReadTextFileRequest::new(SessionId::from(SESSION), PathBuf::from("/missing.txt"));
    let result = runtime.handle_read_text_file(SESSION, req).await;
    assert!(result.is_err(), "reading a missing file should fail");
}

#[tokio::test]
async fn path_traversal_rejected_without_touching_fs() {
    let fs = MockFs::new();
    let runtime = WasmRuntime::with_services(
        &mock_config(PathBuf::from("/sessions")),
        None::<MockNatsBroker>,
        fs.clone(),
        MockProcessSpawner::new(),
        MockWasmExecutor::success(),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    let req = WriteTextFileRequest::new(
        SessionId::from(SESSION),
        PathBuf::from("/../escape.txt"),
        "bad",
    );
    let result = runtime.handle_write_text_file(SESSION, req).await;
    assert!(result.is_err(), "path traversal should be rejected");
    // Nothing should have been written to MockFs.
    assert!(
        fs.files.lock().unwrap().is_empty(),
        "no file should be written on traversal attempt"
    );
}

#[tokio::test]
async fn cleanup_stale_sessions_uses_mock_fs() {
    let fs = MockFs::new();
    let root = PathBuf::from("/sessions");

    // Pre-seed a stale session directory.
    let stale = root.join("stale-session");
    fs.create_dir_all(&stale).await.unwrap();

    let runtime = WasmRuntime::with_services(
        &mock_config(root.clone()),
        None::<MockNatsBroker>,
        fs.clone(),
        MockProcessSpawner::new(),
        MockWasmExecutor::success(),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    runtime.cleanup_stale_sessions().await;

    // The stale dir should have been removed from MockFs.
    assert!(
        !fs.dir_exists(&stale),
        "stale session directory should have been cleaned up"
    );
}

// ── native terminal tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn create_native_terminal_uses_mock_spawner() {
    let spawner = MockProcessSpawner::new();
    let runtime = WasmRuntime::with_services(
        &mock_config(PathBuf::from("/sessions")),
        None::<MockNatsBroker>,
        MockFs::new(),
        spawner.clone(),
        MockWasmExecutor::success(),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let req = CreateTerminalRequest::new(SessionId::from(SESSION), "/bin/echo");
            let resp = runtime
                .handle_create_terminal(SESSION, req)
                .await
                .expect("create terminal should succeed");

            // A terminal ID was returned.
            assert!(
                !resp.terminal_id.0.as_ref().is_empty(),
                "terminal_id should not be empty"
            );
            // The spawner recorded the command.
            assert_eq!(
                spawner.spawned.lock().unwrap().as_slice(),
                &["/bin/echo"],
                "spawner should have recorded the command"
            );
        })
        .await;
}

#[tokio::test]
async fn wasm_only_rejects_native_without_touching_spawner() {
    let spawner = MockProcessSpawner::new();
    let mut config = mock_config(PathBuf::from("/sessions"));
    config.wasm_only = true;

    let runtime = WasmRuntime::with_services(
        &config,
        None::<MockNatsBroker>,
        MockFs::new(),
        spawner.clone(),
        MockWasmExecutor::success(),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let req = CreateTerminalRequest::new(SessionId::from(SESSION), "/bin/echo");
            let result = runtime.handle_create_terminal(SESSION, req).await;
            assert!(result.is_err(), "native command should be rejected in wasm_only mode");
            assert!(
                spawner.spawned.lock().unwrap().is_empty(),
                "spawner should not be called when command is rejected"
            );
        })
        .await;
}

// ── WASM terminal tests (using MockWasmExecutor) ──────────────────────────────

#[tokio::test]
async fn wasm_terminal_output_uses_mock_executor() {
    let runtime = WasmRuntime::with_services(
        &mock_config(PathBuf::from("/sessions")),
        None::<MockNatsBroker>,
        MockFs::new(),
        MockProcessSpawner::new(),
        MockWasmExecutor::success(),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // Use an absolute path ending in .wasm so runtime routes to WASM path.
            let req = CreateTerminalRequest::new(SessionId::from(SESSION), "/fake/module.wasm");
            let resp = runtime
                .handle_create_terminal(SESSION, req)
                .await
                .expect("WASM terminal creation should succeed");

            let terminal_id = resp.terminal_id.clone();
            assert!(
                !terminal_id.0.as_ref().is_empty(),
                "terminal_id should not be empty"
            );

            // Wait for the mock executor to complete.
            let wait_req = WaitForTerminalExitRequest::new(
                agent_client_protocol::SessionId::from(SESSION),
                terminal_id.clone(),
            );
            let exit_resp = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit_resp.exit_status.exit_code,
                Some(0),
                "mock executor should exit with code 0"
            );

            // Verify mock output is present in terminal snapshot.
            let output_req = agent_client_protocol::TerminalOutputRequest::new(
                agent_client_protocol::SessionId::from(SESSION),
                terminal_id,
            );
            let output_resp = runtime
                .handle_terminal_output(output_req)
                .await
                .expect("output should be available");
            assert!(
                output_resp.output.contains("mock wasm output"),
                "output should contain mock wasm output, got: {:?}",
                output_resp.output
            );
        })
        .await;
}

#[tokio::test]
async fn wasm_terminal_nonzero_exit_is_captured() {
    let runtime = WasmRuntime::with_services(
        &mock_config(PathBuf::from("/sessions")),
        None::<MockNatsBroker>,
        MockFs::new(),
        MockProcessSpawner::new(),
        MockWasmExecutor::with_exit(42),
        MockClock::new(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let req = CreateTerminalRequest::new(SessionId::from(SESSION), "/fake/module.wasm");
            let resp = runtime
                .handle_create_terminal(SESSION, req)
                .await
                .expect("WASM terminal creation should succeed");

            let wait_req = WaitForTerminalExitRequest::new(
                agent_client_protocol::SessionId::from(SESSION),
                resp.terminal_id,
            );
            let exit_resp = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit_resp.exit_status.exit_code,
                Some(42),
                "exit code should be 42"
            );
        })
        .await;
}

// ── MockClock / MockIdGenerator tests ────────────────────────────────────────

#[tokio::test]
async fn mock_clock_controls_idle_timeout() {
    let clock = MockClock::new();
    let mut config = mock_config(PathBuf::from("/sessions"));
    config.session_idle_timeout_secs = 3600;

    let runtime = WasmRuntime::with_services(
        &config,
        None::<MockNatsBroker>,
        MockFs::new(),
        MockProcessSpawner::new(),
        MockWasmExecutor::success(),
        clock.clone(),
        MockIdGenerator::new(),
        UnlimitedTaskLimiter,
    )
    .unwrap();

    // Trigger session creation via a write.
    let req = WriteTextFileRequest::new(
        SessionId::from(SESSION),
        PathBuf::from("/probe.txt"),
        "data",
    );
    runtime.handle_write_text_file(SESSION, req).await.unwrap();
    assert_eq!(runtime.list_sessions(), vec![SESSION.to_string()]);

    // Advance clock by 2 hours — session is now idle past the 1-hour threshold.
    clock.advance(std::time::Duration::from_secs(7200));

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async { runtime.cleanup_idle_sessions() })
        .await;

    assert!(
        runtime.list_sessions().is_empty(),
        "session should be cleaned up after mock clock advance"
    );
}

#[tokio::test]
async fn mock_id_generator_produces_predictable_terminal_ids() {
    let id_gen = MockIdGenerator::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::with_services(
                &mock_config(PathBuf::from("/sessions")),
                None::<MockNatsBroker>,
                MockFs::new(),
                MockProcessSpawner::new(),
                MockWasmExecutor::success(),
                MockClock::new(),
                id_gen,
                UnlimitedTaskLimiter,
            )
            .unwrap();

            let req = CreateTerminalRequest::new(
                SessionId::from(SESSION),
                "/bin/echo",
            );
            let resp = runtime
                .handle_create_terminal(SESSION, req)
                .await
                .expect("should succeed");

            assert_eq!(
                resp.terminal_id.0.as_ref(),
                "mock-id-1",
                "MockIdGenerator should produce predictable IDs"
            );
        })
        .await;
}
