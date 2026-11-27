//! Limits and defaults shared across the server.

/// Output a terminal session keeps before it drops the oldest bytes.
pub(crate) const MAX_BUFFERED_BYTES: usize = 256 * 1024;
/// Longest a terminal read waits for output.
pub(crate) const MAX_READ_WAIT_MS: u32 = 20_000;
/// What a terminal read returns at most when the caller sets no limit.
pub(crate) const DEFAULT_READ_BYTES: usize = 64 * 1024;
/// What an unauthenticated tool call tells the model to do.
pub(crate) const UNAUTHENTICATED_MSG: &str =
    "Unauthenticated: set --api-key, SANDBOX_API_KEY, or ~/.config/trogon-sandbox-mcp/api-key (mode 0600)";
/// The only mode an API key file may have.
pub(crate) const API_KEY_FILE_MODE: u32 = 0o600;
/// The API key file's location below the home directory.
pub(crate) const API_KEY_PATH_SEGMENTS: [&str; 3] = [".config", "trogon-sandbox-mcp", "api-key"];
/// The logger name evidence notifications carry.
pub(crate) const EVIDENCE_LOGGER: &str = "sandbox.evidence";
/// How long a lease request waits for the broker.
pub(crate) const LEASE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// How often a progress run polls for its command's result.
pub(crate) const COMPLETION_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// How long past its own timeout a progress run waits for its command's result.
pub(crate) const COMPLETION_GRACE_SECONDS: u64 = 30;
/// The width of a terminal opened without one.
pub(crate) const DEFAULT_TERMINAL_COLS: u16 = 120;
/// The height of a terminal opened without one.
pub(crate) const DEFAULT_TERMINAL_ROWS: u16 = 40;
