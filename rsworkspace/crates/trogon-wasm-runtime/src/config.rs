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
    /// Directory for on-disk compiled module cache (.cwasm files).
    /// When `None`, on-disk caching is disabled (in-memory cache still active).
    pub module_cache_dir: Option<PathBuf>,
    /// When `true`, WASM modules can access the network via WASI sockets.
    /// Defaults to `false`.
    pub wasm_allow_network: bool,
    /// Maximum fuel (instruction budget) for a single WASM module execution.
    /// Defaults to 1 billion. Set to 0 to use u64::MAX (effectively unlimited
    /// but the engine still has consume_fuel enabled).
    pub wasm_fuel_limit: u64,
    /// Maximum number of trogon.* host function calls per WASM module execution.
    /// When exhausted, host functions silently return -1 without executing.
    /// Defaults to 10_000.
    pub wasm_host_call_limit: u32,
    /// ACP subject prefix used for WASM permission requests over NATS.
    pub acp_prefix: String,
    /// Maximum number of concurrent WASM terminal executions across all sessions.
    /// Defaults to 32. Set to 0 for unlimited (not recommended).
    pub wasm_max_concurrent_tasks: usize,
    /// How long (in seconds) a session can be idle before it is automatically cleaned up.
    /// Defaults to 3600 (1 hour). Set to 0 to disable automatic expiry.
    pub session_idle_timeout_secs: u64,
    /// Maximum allowed size for a `.wasm` module file in bytes.
    /// Defaults to 100 MB. Set to 0 for unlimited (not recommended).
    pub wasm_max_module_size_bytes: usize,
}

const DEFAULT_SESSION_ROOT: &str = "/tmp/trogon-wasm-runtime";
const DEFAULT_OUTPUT_BYTE_LIMIT: usize = 10 * 1024 * 1024; // 10 MB
const ENV_SESSION_ROOT: &str = "WASM_SESSION_ROOT";
const ENV_OUTPUT_BYTE_LIMIT: &str = "WASM_OUTPUT_BYTE_LIMIT";
const ENV_AUTO_ALLOW_PERMISSIONS: &str = "WASM_AUTO_ALLOW_PERMISSIONS";
const ENV_WASM_TIMEOUT_SECS: &str = "WASM_TIMEOUT_SECS";
const ENV_WASM_ONLY: &str = "WASM_ONLY";
const ENV_WASM_MEMORY_LIMIT_BYTES: &str = "WASM_MEMORY_LIMIT_BYTES";
const ENV_MODULE_CACHE_DIR: &str = "WASM_MODULE_CACHE_DIR";
const ENV_WASM_ALLOW_NETWORK: &str = "WASM_ALLOW_NETWORK";
const ENV_WASM_FUEL_LIMIT: &str = "WASM_FUEL_LIMIT";
const ENV_WASM_HOST_CALL_LIMIT: &str = "WASM_HOST_CALL_LIMIT";
const ENV_ACP_PREFIX: &str = "ACP_PREFIX";
const ENV_WASM_MAX_CONCURRENT_TASKS: &str = "WASM_MAX_CONCURRENT_TASKS";
const ENV_SESSION_IDLE_TIMEOUT_SECS: &str = "SESSION_IDLE_TIMEOUT_SECS";
const ENV_WASM_MAX_MODULE_SIZE_BYTES: &str = "WASM_MAX_MODULE_SIZE_BYTES";

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
            .unwrap_or(true);

        let wasm_memory_limit_bytes = std::env::var(ENV_WASM_MEMORY_LIMIT_BYTES)
            .ok()
            .and_then(|s| s.parse().ok());

        let module_cache_dir = std::env::var(ENV_MODULE_CACHE_DIR)
            .ok()
            .map(PathBuf::from);

        let wasm_allow_network = std::env::var(ENV_WASM_ALLOW_NETWORK)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let wasm_fuel_limit = std::env::var(ENV_WASM_FUEL_LIMIT)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1_000_000_000u64);

        let wasm_host_call_limit = std::env::var(ENV_WASM_HOST_CALL_LIMIT)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_000u32);

        let acp_prefix = std::env::var(ENV_ACP_PREFIX)
            .unwrap_or_else(|_| acp_nats::DEFAULT_ACP_PREFIX.to_string());

        let wasm_max_concurrent_tasks = std::env::var(ENV_WASM_MAX_CONCURRENT_TASKS)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(32usize);

        let session_idle_timeout_secs = std::env::var(ENV_SESSION_IDLE_TIMEOUT_SECS)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600u64);

        let wasm_max_module_size_bytes = std::env::var(ENV_WASM_MAX_MODULE_SIZE_BYTES)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100 * 1024 * 1024usize);

        Self {
            session_root,
            output_byte_limit,
            auto_allow_permissions,
            wasm_timeout_secs,
            wasm_only,
            wasm_memory_limit_bytes,
            module_cache_dir,
            wasm_allow_network,
            wasm_fuel_limit,
            wasm_host_call_limit,
            acp_prefix,
            wasm_max_concurrent_tasks,
            session_idle_timeout_secs,
            wasm_max_module_size_bytes,
        }
    }

    /// Validates configuration values. Returns a list of errors found.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.output_byte_limit == 0 {
            errors.push("WASM_OUTPUT_BYTE_LIMIT must be > 0".to_string());
        }
        if !self.session_root.exists() {
            if let Err(e) = std::fs::create_dir_all(&self.session_root) {
                errors.push(format!(
                    "Cannot create session_root {:?}: {e}",
                    self.session_root
                ));
            }
        }
        if let Some(ref dir) = self.module_cache_dir {
            if let Err(e) = std::fs::create_dir_all(dir) {
                errors.push(format!("Cannot create module_cache_dir {:?}: {e}", dir));
            }
        }
        errors
    }
}
