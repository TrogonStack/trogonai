use std::time::Duration;

/// Native options this worker accepts under `native.local_docker`.
pub(crate) const NATIVE_OPTION_KEYS: &[&str] = &["pids_limit", "shm_size_mib"];

/// Most output a readiness probe can look back over, per execution.
pub(crate) const PROCESS_OUTPUT_TAIL: usize = 256 * 1024;

/// How often a readiness wait re-evaluates its probe.
pub(crate) const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long stop waits, after SIGKILL, for the execution to be recorded as
/// finished.
pub(crate) const STOP_KILL_SETTLE: Duration = Duration::from_secs(5);

pub(crate) const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Succeeds when something listens on the TCP port `$1` (four hex digits)
/// in this network namespace. Reads the kernel table, so it needs only `grep`.
pub(crate) const TCP_LISTEN_SCRIPT: &str =
    r#"grep -qiE "^ *[0-9]+: [0-9A-F]+:$1 [0-9A-F]+:[0-9A-F]+ 0A " /proc/net/tcp /proc/net/tcp6 2>/dev/null"#;

/// How long a stdin write waits for the process to accept it before the
/// caller is told to retry with the same sequence number.
pub(crate) const STDIN_WRITE_DEADLINE: Duration = Duration::from_secs(5);

/// How long a write the process already started may take to finish before
/// stdin is declared broken.
pub(crate) const STDIN_STARTED_WRITE_DEADLINE: Duration = Duration::from_secs(30);

/// How long kill waits for a just-started exec to record its Docker exec id.
pub(crate) const EXEC_ID_WAIT: Duration = Duration::from_secs(2);

pub(crate) const EXEC_ID_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Writes queued toward the exec's stdin before callers see backpressure.
pub(crate) const STDIN_QUEUE_DEPTH: usize = 16;

/// How much of stderr a [`OutputRetention::StreamOnly`] execution keeps for
/// `poll_command`.
pub(crate) const STREAM_ONLY_STDERR_TAIL: usize = 64 * 1024;

pub(crate) const EXECUTION_ID_ENV: &str = trogon_sandbox_contract::CommandEnv::RESERVED_KEY;

pub(crate) const SIGNAL_EXEC_USER: &str = "0";

pub(crate) const OUTPUT_SETTLE_GRACE: Duration = Duration::from_secs(5);

pub(crate) const CONTAINER_DISPOSAL_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) const OP_QUEUED: u8 = 0;

pub(crate) const OP_STARTED: u8 = 1;

pub(crate) const OP_CANCELLED: u8 = 2;

pub(crate) const COMMAND_CUSTODY_BUCKET: &str = "sandbox-docker-command-custody";

pub(crate) const MAX_CUSTODY_RECORD_BYTES: usize = 8192;

pub(crate) const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

pub(crate) const OWNER_LABEL: &str = "sandbox.trogon.live/owner";

pub(crate) const DIGEST_LABEL: &str = "sandbox.trogon.live/command-runtime-digest";

pub(crate) const NONCE_LABEL: &str = "sandbox.trogon.live/command-runtime-registration";

pub(crate) const RUNTIME_FILE_NAME: &str = "supervisor";

/// Region an S3 volume uses when the worker is not configured with one.
pub(crate) const DEFAULT_VOLUME_REGION: &str = "us-east-1";
