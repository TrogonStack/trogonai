pub(crate) const DEFAULT_S3_REGION: &str = "us-east-1";

pub(crate) const ROOTFS_LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

pub(crate) const S3_ENDPOINT_ENV: &str = "SANDBOX_S3_ENDPOINT";

pub(crate) const S3_KMS_KEY_ID_ENV: &str = "SANDBOX_S3_KMS_KEY_ID";

pub(crate) const S3_REGION_ENV: &str = "SANDBOX_S3_REGION";

pub(crate) const POD_PHASE_FAILED: &str = "Failed";

pub(crate) const POD_PHASE_RUNNING: &str = "Running";

pub(crate) const POD_PHASE_SUCCEEDED: &str = "Succeeded";

pub(crate) const WARM_POOL_RECONCILE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) const WARM_RESTORE_SECRET_PREFIX: &str = "warm-restore-";

/// The largest CPU request a lease may name, in the wire's millis. A pod
/// asking for more than a node has stays Pending until the lease times out.
pub(crate) const MAX_VCPU_MILLIS: u32 = 8000;

/// The smallest CPU request a lease may name, in the wire's millis.
pub(crate) const MIN_VCPU_MILLIS: u32 = 100;

/// How long a stdin write waits for the process to accept it before the
/// caller is told to retry with the same sequence number.
pub(crate) const STDIN_WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a write the process already started may take to finish before
/// stdin is declared broken.
pub(crate) const STDIN_STARTED_WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Writes queued toward the exec's stdin before callers see backpressure.
pub(crate) const STDIN_QUEUE_DEPTH: usize = 16;

pub(crate) const STDIN_OP_QUEUED: u8 = 0;
pub(crate) const STDIN_OP_STARTED: u8 = 1;
pub(crate) const STDIN_OP_CANCELLED: u8 = 2;

/// How long a timed-out command may take to report completion after its kill signal was accepted.
pub(crate) const COMMAND_COMPLETION_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a signal waits for a just-started command to show its marker in
/// the pod before the command is reported as not found.
pub(crate) const MARKER_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) const MARKER_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// How long stop waits, after SIGKILL, for the execution to be recorded as finished.
pub(crate) const STOP_KILL_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) const STOP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// How long a cold lease waits for its pod, image pull included.
pub(crate) const POD_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How long a rollback waits for the replaced sandbox to disappear.
pub(crate) const OLD_SANDBOX_GONE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How often the worker sweeps expired lease Jobs.
pub(crate) const LEASE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// The terminal type a new terminal gets when the request names none.
pub(crate) const DEFAULT_TERM: &str = "xterm-256color";

/// How long a terminal input write may take before the terminal is reported as exited.
pub(crate) const INPUT_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A terminal with no input and no output for this long is closed.
pub(crate) const DEFAULT_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// How long a terminal whose output ended waits for the exec to report its exit code.
pub(crate) const EXIT_CODE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The queue group that spreads lease dispatches across the workers of this provider kind.
pub(crate) const LEASE_QUEUE_GROUP: &str = "sandbox-provider-k8s_micro_vm";

/// The mode a written file gets when the request names none.
pub(crate) const DEFAULT_FILE_MODE: u32 = 0o644;
