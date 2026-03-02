use std::time::Duration;
use tracing::warn;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::acp_prefix::AcpPrefix;
use crate::constants::{
    DEFAULT_CONNECT_TIMEOUT_SECS, DEFAULT_MAX_CONCURRENT_CLIENT_TASKS, DEFAULT_OPERATION_TIMEOUT,
    DEFAULT_PROMPT_TIMEOUT, ENV_CONNECT_TIMEOUT_SECS, ENV_OPERATION_TIMEOUT_SECS,
    ENV_PROMPT_TIMEOUT_SECS, MIN_TIMEOUT_SECS,
};

pub use crate::constants::{DEFAULT_ACP_PREFIX, ENV_ACP_PREFIX};

#[derive(Clone)]
pub struct Config {
    pub(crate) acp_prefix: AcpPrefix,
    pub(crate) nats: NatsConfig,
    pub(crate) operation_timeout: Duration,
    pub(crate) prompt_timeout: Duration,
    pub(crate) max_concurrent_client_tasks: usize,
}

impl Config {
    pub fn new(acp_prefix: AcpPrefix, nats: NatsConfig) -> Self {
        Self {
            acp_prefix,
            nats,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            prompt_timeout: DEFAULT_PROMPT_TIMEOUT,
            max_concurrent_client_tasks: DEFAULT_MAX_CONCURRENT_CLIENT_TASKS,
        }
    }

    pub fn with_prefix(acp_prefix: AcpPrefix, nats: NatsConfig) -> Self {
        Self::new(acp_prefix, nats)
    }

    pub fn with_operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout;
        self
    }

    pub fn with_prompt_timeout(mut self, timeout: Duration) -> Self {
        self.prompt_timeout = timeout;
        self
    }

    pub fn with_max_concurrent_client_tasks(mut self, max: usize) -> Self {
        self.max_concurrent_client_tasks = max.max(1);
        self
    }

    pub fn acp_prefix(&self) -> &str {
        self.acp_prefix.as_str()
    }

    pub fn acp_prefix_ref(&self) -> &AcpPrefix {
        &self.acp_prefix
    }

    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }

    pub fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    /// Returns the configured timeout for prompt requests.
    pub fn prompt_timeout(&self) -> Duration {
        self.prompt_timeout
    }

    /// Returns the maximum concurrent client tasks (backpressure limit).
    ///
    /// A minimum of 1 avoids misconfiguration that would permanently reject all client messages.
    pub fn max_concurrent_client_tasks(&self) -> usize {
        self.max_concurrent_client_tasks
    }

    #[cfg(test)]
    pub(crate) fn for_test(acp_prefix: &str) -> Self {
        let nats = NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        };
        Self::new(AcpPrefix::new(acp_prefix.to_string()).unwrap(), nats)
            .with_prompt_timeout(crate::constants::TEST_PROMPT_TIMEOUT)
    }
}

pub fn apply_timeout_overrides<E: ReadEnv>(config: Config, env_provider: &E) -> Config {
    let mut config = config;

    if let Ok(raw) = env_provider.var(ENV_OPERATION_TIMEOUT_SECS) {
        match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => {
                config = config.with_operation_timeout(Duration::from_secs(secs));
            }
            Ok(secs) => {
                warn!(
                    "{ENV_OPERATION_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
            }
            Err(_) => {
                warn!("{ENV_OPERATION_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
            }
        }
    }

    if let Ok(raw) = env_provider.var(ENV_PROMPT_TIMEOUT_SECS) {
        match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => {
                config = config.with_prompt_timeout(Duration::from_secs(secs));
            }
            Ok(secs) => {
                warn!(
                    "{ENV_PROMPT_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
            }
            Err(_) => {
                warn!("{ENV_PROMPT_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
            }
        }
    }

    config
}

pub fn nats_connect_timeout<E: ReadEnv>(env_provider: &E) -> Duration {
    let default = Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS);

    match env_provider.var(ENV_CONNECT_TIMEOUT_SECS) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => Duration::from_secs(secs),
            Ok(secs) => {
                warn!(
                    "{ENV_CONNECT_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
                default
            }
            Err(_) => {
                warn!("{ENV_CONNECT_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
                default
            }
        },
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn default_nats() -> NatsConfig {
        NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        }
    }

    #[test]
    fn config_new_accepts_validated_prefix() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
        assert_eq!(config.acp_prefix(), "acp");
    }

    #[test]
    fn config_with_operation_timeout() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats())
            .with_operation_timeout(Duration::from_secs(60));
        assert_eq!(config.operation_timeout(), Duration::from_secs(60));
    }

    #[test]
    fn config_with_prompt_timeout() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats())
            .with_prompt_timeout(Duration::from_secs(3600));
        assert_eq!(config.prompt_timeout(), Duration::from_secs(3600));
    }

    #[test]
    fn config_with_max_concurrent_client_tasks() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats())
            .with_max_concurrent_client_tasks(32);
        assert_eq!(config.max_concurrent_client_tasks(), 32);
    }

    #[test]
    fn config_default_max_concurrent_client_tasks_is_256() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
        assert_eq!(config.max_concurrent_client_tasks(), 256);
    }

    #[test]
    fn config_with_max_concurrent_client_tasks_enforces_minimum() {
        let config = Config::for_test("acp").with_max_concurrent_client_tasks(0);
        assert_eq!(config.max_concurrent_client_tasks(), 1);
    }

    #[test]
    fn config_nats_returns_nats_config() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
        assert_eq!(config.nats().servers.len(), 1);
        assert_eq!(config.nats().servers[0], "localhost:4222");
    }

    #[test]
    fn config_with_prefix_accepts_validated_prefix() {
        let config = Config::with_prefix(AcpPrefix::new("acp").unwrap(), default_nats());
        assert_eq!(config.acp_prefix(), "acp");
    }

    fn with_subscriber<F: FnOnce()>(f: F) {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = tracing_subscriber::fmt().with_test_writer().set_default();
        f();
    }

    #[test]
    fn test_nats_connect_timeout_from_env() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );

            env.set(ENV_CONNECT_TIMEOUT_SECS, "15");
            assert_eq!(nats_connect_timeout(&env), Duration::from_secs(15));
            env.set(ENV_CONNECT_TIMEOUT_SECS, "0");
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );
            env.set(ENV_CONNECT_TIMEOUT_SECS, "not-a-number");
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );
        });
    }

    #[test]
    fn test_operation_timeout_invalid_env_is_ignored() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            let default = Config::for_test("acp");
            let default_timeout = default.operation_timeout();

            env.set("ACP_OPERATION_TIMEOUT_SECS", "not-a-number");
            let invalid = apply_timeout_overrides(Config::for_test("acp"), &env);
            assert_eq!(invalid.operation_timeout(), default_timeout);
        });
    }

    #[test]
    fn test_operation_timeout_under_min_is_ignored() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            let default = Config::for_test("acp");
            let default_timeout = default.operation_timeout();

            env.set("ACP_OPERATION_TIMEOUT_SECS", "0");
            let ignored = apply_timeout_overrides(Config::for_test("acp"), &env);
            assert_eq!(ignored.operation_timeout(), default_timeout);
        });
    }

    #[test]
    fn test_operation_timeout_valid_override() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            env.set("ACP_OPERATION_TIMEOUT_SECS", "60");
            let config = apply_timeout_overrides(Config::for_test("acp"), &env);
            assert_eq!(config.operation_timeout(), Duration::from_secs(60));
        });
    }

    #[test]
    fn test_prompt_timeout_invalid_env_is_ignored() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            let default = Config::for_test("acp");
            let default_timeout = default.prompt_timeout();

            env.set("ACP_PROMPT_TIMEOUT_SECS", "not-a-number");
            let invalid = apply_timeout_overrides(Config::for_test("acp"), &env);
            assert_eq!(invalid.prompt_timeout(), default_timeout);
        });
    }

    #[test]
    fn test_prompt_timeout_under_min_is_ignored() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            let default = Config::for_test("acp");
            let default_timeout = default.prompt_timeout();

            env.set("ACP_PROMPT_TIMEOUT_SECS", "0");
            let ignored = apply_timeout_overrides(Config::for_test("acp"), &env);
            assert_eq!(ignored.prompt_timeout(), default_timeout);
        });
    }

    #[test]
    fn test_prompt_timeout_valid_override() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            env.set("ACP_PROMPT_TIMEOUT_SECS", "3600");
            let config = apply_timeout_overrides(Config::for_test("acp"), &env);
            assert_eq!(config.prompt_timeout(), Duration::from_secs(3600));
        });
    }

    /// `char::is_whitespace()` covers Unicode whitespace (e.g. U+00A0 NO-BREAK
    /// SPACE) in addition to ASCII whitespace.  These characters are not safe
    /// in NATS subjects and must be rejected.
    #[test]
    fn acp_prefix_rejects_unicode_whitespace() {
        let err = AcpPrefix::new("acp\u{00A0}foo").err().unwrap();
        assert!(
            matches!(err, ValidationError::InvalidCharacter("acp_prefix", '\u{00A0}')),
            "expected InvalidCharacter with U+00A0, got: {:?}",
            err
        );
    }

    /// Consecutive dots (`..`) are rejected via `has_consecutive_or_boundary_dots`
    /// and produce `ValidationError::InvalidCharacter(_, '.')`.
    #[test]
    fn acp_prefix_consecutive_dots_returns_invalid_character_dot() {
        let err = AcpPrefix::new("acp..foo").err().unwrap();
        assert!(
            matches!(err, ValidationError::InvalidCharacter("acp_prefix", '.')),
            "expected InvalidCharacter('.'), got: {:?}",
            err
        );
    }
}
