use std::time::Duration;

/// Raw bytes per chunk of a [`SandboxClient::upload_file`](crate::SandboxClient::upload_file) or
/// [`SandboxClient::download_file`](crate::SandboxClient::download_file) transfer. Each chunk is one broker round
/// trip.
pub const FILE_TRANSFER_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// The reply deadline for calls that take no explicit timeout.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Most consecutive failed reads [`SandboxClient::follow_logs`](crate::SandboxClient::follow_logs) absorbs before it
/// gives up.
pub const FOLLOW_LOGS_MAX_RECONNECTS: u32 = 5;
pub(crate) const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DRIVE_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const FORK_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const REAP_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const LISTING_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const DELETE_NAMED_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the client waits for a detached command to start.
pub(crate) const DETACHED_START_TIMEOUT: Duration = Duration::from_secs(40);
pub(crate) const FILE_TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const FILE_OP_TIMEOUT: Duration = Duration::from_secs(60);
/// The reply deadline for lease verbs that only update broker state.
pub(crate) const QUICK_VERB_TIMEOUT: Duration = Duration::from_secs(2);
/// The reply deadline for lease verbs that cover a worker freezing or thawing a sandbox.
pub(crate) const SLOW_VERB_TIMEOUT: Duration = Duration::from_secs(30);
/// How long each [`SandboxClient::follow_logs`] read parks at the broker. Output and the worker's exit marker wake
/// it at once, so `End` follows the exit directly. The bound only matters when no marker arrives (an older worker or
/// broker): it is then how long a silent command's exit can go unnoticed before the follower polls for it.
pub(crate) const FOLLOW_LOGS_WAIT: Duration = Duration::from_secs(1);
pub(crate) const FOLLOW_LOGS_RECONNECT_BACKOFF: Duration = Duration::from_millis(200);
pub(crate) const FOLLOW_LOGS_RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(3);
pub(crate) const CHUNKED_SUBJECT_PREFIXES: [&str; 3] = [
    "sandbox.lease.write_files.",
    "sandbox.lease.read_file.",
    "sandbox.lease.file_op.",
];
pub(crate) const TERMINAL_TIMEOUT: Duration = Duration::from_secs(15);
