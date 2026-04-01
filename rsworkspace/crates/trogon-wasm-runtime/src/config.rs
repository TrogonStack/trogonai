use std::path::PathBuf;

/// Runtime configuration, read from environment variables.
pub struct Config {
    /// Root directory where per-session sandbox directories are created.
    pub session_root: PathBuf,
    /// Maximum number of bytes kept in the output buffer per terminal.
    /// Oldest bytes are dropped when the limit is exceeded.
    pub output_byte_limit: usize,
    /// Permission policy: when `true` the first option is auto-selected.
    /// When `false` all permission requests are rejected (returns `Cancelled`).
    pub auto_allow_permissions: bool,
}

const DEFAULT_SESSION_ROOT: &str = "/tmp/trogon-wasm-runtime";
const DEFAULT_OUTPUT_BYTE_LIMIT: usize = 10 * 1024 * 1024; // 10 MB
const ENV_SESSION_ROOT: &str = "WASM_SESSION_ROOT";
const ENV_OUTPUT_BYTE_LIMIT: &str = "WASM_OUTPUT_BYTE_LIMIT";
const ENV_AUTO_ALLOW_PERMISSIONS: &str = "WASM_AUTO_ALLOW_PERMISSIONS";

impl Config {
    pub fn from_env() -> Self {
        let session_root = std::env::var(ENV_SESSION_ROOT)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_SESSION_ROOT));

        let output_byte_limit = std::env::var(ENV_OUTPUT_BYTE_LIMIT)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_OUTPUT_BYTE_LIMIT);

        let auto_allow_permissions = std::env::var(ENV_AUTO_ALLOW_PERMISSIONS)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        Self {
            session_root,
            output_byte_limit,
            auto_allow_permissions,
        }
    }
}
