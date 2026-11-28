use std::time::Duration;

pub(crate) const APPEND: &str = "printf '%s' \"$2\" | base64 -d >> \"$1\"";

pub(crate) const DEFAULT_CWD: &str = "/workspace";

pub(crate) const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) const DEFAULT_FILE_MODE: u32 = 0o644;

pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(35);

pub(crate) const DISCARD: &str = "rm -f -- \"$1\"";

pub(crate) const EXEC_GRACE: Duration = Duration::from_secs(5);

pub(crate) const FILE_CHUNK: usize = 24 * 1024;

pub(crate) const FINISH: &str = "chmod -- \"$3\" \"$1\" && mv -f -- \"$1\" \"$2\"";

pub(crate) const IMAGE_HASH_HEX_LEN: usize = 16;

pub(crate) const LEDGER_FILE: &str = "dispatch.db";

pub(crate) const MAKE_TEMP: &str = "mkdir -p -- \"$(dirname -- \"$1\")\" && mktemp -- \"$1.trogon.XXXXXX\"";

pub(crate) const MAX_FILES: usize = 256;

pub(crate) const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

pub(crate) const MAX_FILE_MODE: u32 = 0o7777;

/// The most leases one worker may hold.
pub const MAX_LEASES_LIMIT: usize = 128;

pub(crate) const MAX_MIB_PER_CPU: u32 = 2048;

pub(crate) const MAX_TIMEOUT_SECONDS: u32 = 600;

/// The largest vCPU count this worker advertises, in the wire's millis.
pub const MAX_VCPU_MILLIS: u32 = 8000;

pub(crate) const MEMORY_STEP_MIB: u32 = 256;

pub(crate) const META_ATTEMPT: &str = "fly_worker_dispatch_attempt";

pub(crate) const META_LEASE: &str = "fly_worker_lease";

pub(crate) const META_NAME: &str = "fly_worker_attempt";

pub(crate) const META_OWNER: &str = "fly_worker_owner";

pub(crate) const META_WORKSPACE_VOLUME: &str = "fly_worker_workspace_volume";

pub(crate) const MIN_MIB_PER_CPU: u32 = 256;

pub(crate) const MIN_VCPU_MILLIS: u32 = 1000;

pub(crate) const OPERATIONS_SUBJECT: &str = "sandbox.provider.fly.*.*";

pub(crate) const OWNER_FILE: &str = "owner";

pub(crate) const POLL: Duration = Duration::from_millis(100);

pub(crate) const READ_CHUNK_BYTES: usize = 24 * 1024;

pub(crate) const READ_CHUNK_SCRIPT: &str = "test -e \"$1\" || exit 3; test -f \"$1\" || exit 1; tmp=$(mktemp) || exit 1; trap 'rm -f -- \"$tmp\"' EXIT; dd if=\"$1\" of=\"$tmp\" bs=24576 skip=\"$2\" count=1 status=none || exit 1; base64 -w 0 \"$tmp\"";

pub(crate) const READ_EXIT_NOT_FOUND: i32 = 3;

pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) const SECONDS_PER_DAY: u64 = 86_400;

pub(crate) const SHARED_CPUS: [u32; 4] = [1, 2, 4, 8];

pub(crate) const STATE_LOCK_FILE: &str = "worker.lock";

pub(crate) const VERSION: &str = "fly1";
