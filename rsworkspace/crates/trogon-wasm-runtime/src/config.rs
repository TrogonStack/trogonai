use std::path::PathBuf;

/// Runtime configuration, read from environment variables.
#[derive(Clone)]
pub struct Config {
    /// Root directory where per-session sandbox directories are created.
    pub session_root: PathBuf,
    /// Maximum number of bytes kept in the output buffer per terminal.
    /// Oldest bytes are dropped when the limit is exceeded.
    pub output_byte_limit: usize,
    /// Permission policy: when `true` the first option is auto-selected without
    /// prompting. When `false` and a NATS client is configured, the request is
    /// forwarded to the user via NATS for an explicit selection. When `false`
    /// with no NATS client, the request is denied immediately.
    pub auto_allow_permissions: bool,
    /// Optional wall-clock timeout for WASM module execution in seconds.
    /// When `None`, no timeout is applied (fuel exhaustion still applies).
    pub wasm_timeout_secs: Option<u64>,
    /// When `true`, only `.wasm` commands are accepted; native OS process
    /// spawning is rejected with an error. Defaults to `true`.
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
    /// Maximum seconds to wait for a terminal to exit before killing it.
    /// Defaults to 300 (5 minutes). Set to 0 for no timeout (not recommended).
    pub wait_for_exit_timeout_secs: u64,
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
const ENV_WAIT_FOR_EXIT_TIMEOUT_SECS: &str = "WAIT_FOR_EXIT_TIMEOUT_SECS";

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
            .unwrap_or(false);

        let wasm_timeout_secs = std::env::var(ENV_WASM_TIMEOUT_SECS)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&v| v > 0);

        let wasm_only = std::env::var(ENV_WASM_ONLY)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let wasm_memory_limit_bytes = std::env::var(ENV_WASM_MEMORY_LIMIT_BYTES)
            .ok()
            .and_then(|s| s.parse().ok());

        let module_cache_dir = std::env::var(ENV_MODULE_CACHE_DIR).ok().map(PathBuf::from);

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

        let wait_for_exit_timeout_secs = std::env::var(ENV_WAIT_FOR_EXIT_TIMEOUT_SECS)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300u64);

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
            wait_for_exit_timeout_secs,
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

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env-var mutations are process-global. Serialize all config tests with
    // a static lock so parallel test threads don't interfere with each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_all_vars() {
        let vars = [
            "WASM_SESSION_ROOT", "WASM_OUTPUT_BYTE_LIMIT", "WASM_AUTO_ALLOW_PERMISSIONS",
            "WASM_TIMEOUT_SECS", "WASM_ONLY", "WASM_MEMORY_LIMIT_BYTES",
            "WASM_MODULE_CACHE_DIR", "WASM_ALLOW_NETWORK", "WASM_FUEL_LIMIT",
            "WASM_HOST_CALL_LIMIT", "ACP_PREFIX", "WASM_MAX_CONCURRENT_TASKS",
            "SESSION_IDLE_TIMEOUT_SECS", "WASM_MAX_MODULE_SIZE_BYTES",
            "WAIT_FOR_EXIT_TIMEOUT_SECS",
        ];
        for v in &vars {
            unsafe { std::env::remove_var(v); }
        }
    }

    #[test]
    fn defaults_when_no_env_vars_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        let cfg = Config::from_env();
        assert_eq!(cfg.session_root, std::path::PathBuf::from("/tmp/trogon-wasm-runtime"));
        assert_eq!(cfg.output_byte_limit, 10 * 1024 * 1024);
        assert!(!cfg.auto_allow_permissions);
        assert_eq!(cfg.wasm_timeout_secs, None);
        assert!(cfg.wasm_only);
        assert_eq!(cfg.wasm_memory_limit_bytes, None);
        assert_eq!(cfg.module_cache_dir, None);
        assert!(!cfg.wasm_allow_network);
        assert_eq!(cfg.wasm_fuel_limit, 1_000_000_000);
        assert_eq!(cfg.wasm_host_call_limit, 10_000);
        assert_eq!(cfg.wasm_max_concurrent_tasks, 32);
        assert_eq!(cfg.session_idle_timeout_secs, 3600);
        assert_eq!(cfg.wasm_max_module_size_bytes, 100 * 1024 * 1024);
        assert_eq!(cfg.wait_for_exit_timeout_secs, 300);
    }

    #[test]
    fn session_root_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_SESSION_ROOT", "/custom/root"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_SESSION_ROOT"); }
        assert_eq!(cfg.session_root, std::path::PathBuf::from("/custom/root"));
    }

    #[test]
    fn output_byte_limit_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_OUTPUT_BYTE_LIMIT", "4096"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_OUTPUT_BYTE_LIMIT"); }
        assert_eq!(cfg.output_byte_limit, 4096);
    }

    #[test]
    fn auto_allow_permissions_true_from_one() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_AUTO_ALLOW_PERMISSIONS", "1"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_AUTO_ALLOW_PERMISSIONS"); }
        assert!(cfg.auto_allow_permissions);
    }

    #[test]
    fn auto_allow_permissions_true_from_true_word() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_AUTO_ALLOW_PERMISSIONS", "True"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_AUTO_ALLOW_PERMISSIONS"); }
        assert!(cfg.auto_allow_permissions);
    }

    #[test]
    fn auto_allow_permissions_false_from_zero() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_AUTO_ALLOW_PERMISSIONS", "0"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_AUTO_ALLOW_PERMISSIONS"); }
        assert!(!cfg.auto_allow_permissions);
    }

    #[test]
    fn wasm_timeout_secs_zero_treated_as_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_TIMEOUT_SECS", "0"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_TIMEOUT_SECS"); }
        assert_eq!(cfg.wasm_timeout_secs, None);
    }

    #[test]
    fn wasm_timeout_secs_positive_is_some() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_TIMEOUT_SECS", "30"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_TIMEOUT_SECS"); }
        assert_eq!(cfg.wasm_timeout_secs, Some(30));
    }

    #[test]
    fn wasm_only_false_from_zero() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_ONLY", "0"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_ONLY"); }
        assert!(!cfg.wasm_only);
    }

    #[test]
    fn wasm_only_default_is_true() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        let cfg = Config::from_env();
        assert!(cfg.wasm_only);
    }

    #[test]
    fn wasm_memory_limit_bytes_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_MEMORY_LIMIT_BYTES", "67108864"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_MEMORY_LIMIT_BYTES"); }
        assert_eq!(cfg.wasm_memory_limit_bytes, Some(67108864));
    }

    #[test]
    fn module_cache_dir_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_MODULE_CACHE_DIR", "/tmp/module-cache"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_MODULE_CACHE_DIR"); }
        assert_eq!(cfg.module_cache_dir, Some(std::path::PathBuf::from("/tmp/module-cache")));
    }

    #[test]
    fn wasm_allow_network_true_from_one() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_ALLOW_NETWORK", "1"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_ALLOW_NETWORK"); }
        assert!(cfg.wasm_allow_network);
    }

    #[test]
    fn fuel_limit_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_FUEL_LIMIT", "500000000"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_FUEL_LIMIT"); }
        assert_eq!(cfg.wasm_fuel_limit, 500_000_000);
    }

    #[test]
    fn host_call_limit_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_HOST_CALL_LIMIT", "1000"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_HOST_CALL_LIMIT"); }
        assert_eq!(cfg.wasm_host_call_limit, 1000);
    }

    #[test]
    fn max_concurrent_tasks_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_MAX_CONCURRENT_TASKS", "8"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_MAX_CONCURRENT_TASKS"); }
        assert_eq!(cfg.wasm_max_concurrent_tasks, 8);
    }

    #[test]
    fn session_idle_timeout_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("SESSION_IDLE_TIMEOUT_SECS", "7200"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("SESSION_IDLE_TIMEOUT_SECS"); }
        assert_eq!(cfg.session_idle_timeout_secs, 7200);
    }

    #[test]
    fn max_module_size_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WASM_MAX_MODULE_SIZE_BYTES", "52428800"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WASM_MAX_MODULE_SIZE_BYTES"); }
        assert_eq!(cfg.wasm_max_module_size_bytes, 52428800);
    }

    #[test]
    fn wait_for_exit_timeout_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        unsafe { std::env::set_var("WAIT_FOR_EXIT_TIMEOUT_SECS", "60"); }
        let cfg = Config::from_env();
        unsafe { std::env::remove_var("WAIT_FOR_EXIT_TIMEOUT_SECS"); }
        assert_eq!(cfg.wait_for_exit_timeout_secs, 60);
    }

    #[test]
    fn validate_returns_error_when_output_byte_limit_is_zero() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        let mut cfg = Config::from_env();
        cfg.output_byte_limit = 0;
        let errors = cfg.validate();
        assert!(
            errors.iter().any(|e| e.contains("WASM_OUTPUT_BYTE_LIMIT")),
            "expected error about output_byte_limit, got: {errors:?}"
        );
    }

    #[test]
    fn validate_returns_no_errors_for_valid_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all_vars();
        let mut cfg = Config::from_env();
        cfg.session_root = std::env::temp_dir().join("trogon-config-test");
        cfg.module_cache_dir = None;
        let errors = cfg.validate();
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }
}
