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
