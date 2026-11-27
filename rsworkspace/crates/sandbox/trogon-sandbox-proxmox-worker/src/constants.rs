use std::time::Duration;

pub(crate) const AGENT_POLL: Duration = Duration::from_millis(250);

pub(crate) const APPEND: &str = "printf '%s' \"$2\" | base64 -d >> \"$1\"";

pub(crate) const ATTEMPTS_DIR: &str = "attempts";

pub(crate) const CAPTURE_MARKER: &str = "trogon-sandbox-capture";

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) const DEFAULT_CWD: &str = "/workspace";

pub(crate) const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) const DEFAULT_FILE_MODE: u32 = 0o644;

pub(crate) const DISCARD: &str = "rm -f -- \"$1\"";

pub(crate) const EXEC_GRACE: Duration = Duration::from_secs(5);

pub(crate) const EXEC_POLL: Duration = Duration::from_millis(100);

pub(crate) const FILE_CHUNK: usize = 24 * 1024;

pub(crate) const FILE_READ_COUNT: &str = "32768";

pub(crate) const FILES_MARKER: &str = "trogon-sandbox-files";

pub(crate) const FINISH: &str = "chmod -- \"$3\" \"$1\" && mv -f -- \"$1\" \"$2\"";

pub(crate) const GUEST_ENV_BIN: &str = "/usr/bin/env";

pub(crate) const GUEST_SHELL_BUDGET: Duration = Duration::from_secs(20);

pub(crate) const GUEST_TIMEOUT_BIN: &str = "/usr/bin/timeout";

pub(crate) const IPFILTER_SET: &str = "ipfilter-net0";

pub(crate) const LEDGER_FILE: &str = "dispatch.db";

pub(crate) const MAKE_TEMP: &str = "mkdir -p -- \"$(dirname -- \"$1\")\" && mktemp -- \"$1.trogon.XXXXXX\"";

pub(crate) const MAX_ATTEMPT_JOURNALS: usize = 4096;

pub(crate) const MAX_FILES: usize = 256;

pub(crate) const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

pub(crate) const MAX_FILE_MODE: u32 = 0o7777;

pub(crate) const MAX_JOURNAL_BYTES: usize = 64 * 1024;

/// The most leases one worker may hold.
pub const MAX_LEASES_LIMIT: usize = 128;

pub(crate) const MAX_LEASE_ID_LEN: usize = 64;

pub(crate) const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub(crate) const MAX_TIMEOUT_SECONDS: u32 = 86_400;

/// The largest vCPU count this worker advertises, in the wire's millis.
pub const MAX_VCPU_MILLIS: u32 = 16_000;

pub(crate) const MAX_VM_ID: u32 = 999_999_999;

/// The smallest vCPU count this worker advertises, in the wire's millis.
pub(crate) const MIN_VCPU_MILLIS: u32 = 100;

pub(crate) const MIN_VM_ID: u32 = 100;

pub(crate) const NO_PROXY: &str = "localhost,127.0.0.1,::1";

pub(crate) const OPERATIONS_SUBJECT: &str = "sandbox.provider.proxmox.*.*";

pub(crate) const OWNER_FILE: &str = "owner";

pub(crate) const PROXY_ENV_KEYS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

pub(crate) const PROXY_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub(crate) const PROXY_USER: &str = "sandbox";

pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the commands and file transfers of the live host check may take together.
pub(crate) const SPIKE_BODY_TIMEOUT: Duration = Duration::from_secs(120);

/// The size of the binary file the live host check writes and reads back.
pub(crate) const SPIKE_FILE_BYTES: usize = 512 * 1024;

/// A guest path with a space, a semicolon and quotes, so file transfer must quote it.
pub(crate) const SPIKE_PATH: &str = "/workspace/spike binary; 'file'";

/// Proves the guest runs as root without a network, keeps the command environment and starts in the workspace,
/// then reports distinct output on both streams and a non-zero exit.
pub(crate) const SPIKE_SCRIPT: &str = "set -eu; test \"$(id -u)\" = 0; test \"$(ls /sys/class/net)\" = lo; \
     test \"$SPIKE_MARKER\" = preserved; test \"$PWD\" = /workspace; printf 'spike-stdout\\n'; \
     printf 'spike-stderr\\n' >&2; exit 7";

pub(crate) const STATE_LOCK_FILE: &str = "worker.lock";

pub(crate) const TASK_POLL: Duration = Duration::from_millis(200);

pub(crate) const VM_NAME_PREFIX: &str = "trogon-sandbox-";

pub(crate) const WORKSPACE_SETUP_BUDGET: Duration = Duration::from_secs(10);

pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(120);
