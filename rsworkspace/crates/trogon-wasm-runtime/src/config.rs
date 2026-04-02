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
    /// Optional wall-clock timeout for WASM module execution in seconds.
    /// When `None`, no timeout is applied (fuel exhaustion still applies).
    pub wasm_timeout_secs: Option<u64>,
    /// When `true`, only `.wasm` commands are accepted; native OS process
    /// spawning is rejected with an error.  Defaults to `false` for
    /// backwards-compatibility but should be `true` in production.
    pub wasm_only: bool,
    /// Optional maximum number of bytes a WASM module's linear memory can grow to.
    /// When `None`, memory growth is unlimited (subject to OS/hardware limits).
    pub wasm_memory_limit_bytes: Option<usize>,
}

const DEFAULT_SESSION_ROOT: &str = "/tmp/trogon-wasm-runtime";
const DEFAULT_OUTPUT_BYTE_LIMIT: usize = 10 * 1024 * 1024; // 10 MB
const ENV_SESSION_ROOT: &str = "WASM_SESSION_ROOT";
const ENV_OUTPUT_BYTE_LIMIT: &str = "WASM_OUTPUT_BYTE_LIMIT";
const ENV_AUTO_ALLOW_PERMISSIONS: &str = "WASM_AUTO_ALLOW_PERMISSIONS";
const ENV_WASM_TIMEOUT_SECS: &str = "WASM_TIMEOUT_SECS";
const ENV_WASM_ONLY: &str = "WASM_ONLY";
const ENV_WASM_MEMORY_LIMIT_BYTES: &str = "WASM_MEMORY_LIMIT_BYTES";

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

        let wasm_timeout_secs = std::env::var(ENV_WASM_TIMEOUT_SECS)
            .ok()
            .and_then(|s| s.parse().ok());

        let wasm_only = std::env::var(ENV_WASM_ONLY)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let wasm_memory_limit_bytes = std::env::var(ENV_WASM_MEMORY_LIMIT_BYTES)
            .ok()
            .and_then(|s| s.parse().ok());

        Self {
            session_root,
            output_byte_limit,
            auto_allow_permissions,
            wasm_timeout_secs,
            wasm_only,
            wasm_memory_limit_bytes,
        }
    }
}
