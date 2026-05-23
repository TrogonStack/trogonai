//! Runtime configuration for the a2a-nats binding.

use std::time::Duration;
use tracing::warn;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::a2a_prefix::A2aPrefix;
use crate::constants::{
    DEFAULT_CONNECT_TIMEOUT_SECS, DEFAULT_MAX_CONCURRENT_CLIENT_TASKS, DEFAULT_OPERATION_TIMEOUT, DEFAULT_TASK_TIMEOUT,
    ENV_CONNECT_TIMEOUT_SECS, ENV_MAX_CONCURRENT_CLIENT_TASKS, ENV_OPERATION_TIMEOUT_SECS, ENV_PUSH_DLQ_CALLER_SEGMENT,
    ENV_TASK_TIMEOUT_SECS, MIN_TIMEOUT_SECS,
};
use crate::push::CallerId;

pub use crate::constants::{DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX};

#[derive(Clone)]
pub struct Config {
    pub(crate) a2a_prefix: A2aPrefix,
    pub(crate) nats: NatsConfig,
    pub(crate) operation_timeout: Duration,
    pub(crate) task_timeout: Duration,
    pub(crate) max_concurrent_client_tasks: usize,
    pub(crate) push_dlq_caller_segment: CallerId,
}

impl Config {
    pub fn new(a2a_prefix: A2aPrefix, nats: NatsConfig) -> Self {
        Self {
            a2a_prefix,
            nats,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            task_timeout: DEFAULT_TASK_TIMEOUT,
            max_concurrent_client_tasks: DEFAULT_MAX_CONCURRENT_CLIENT_TASKS,
            push_dlq_caller_segment: CallerId::default(),
        }
    }

    pub fn with_operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout;
        self
    }

    pub fn with_task_timeout(mut self, timeout: Duration) -> Self {
        self.task_timeout = timeout;
        self
    }

    pub fn with_max_concurrent_client_tasks(mut self, max: usize) -> Self {
        self.max_concurrent_client_tasks = max.max(1);
        self
    }

    pub fn with_push_dlq_caller_segment(mut self, segment: impl Into<String>) -> Self {
        let s = segment.into();
        self.push_dlq_caller_segment = if s.trim().is_empty() {
            CallerId::default()
        } else {
            CallerId::from(s.trim())
        };
        self
    }

    pub fn a2a_prefix(&self) -> &str {
        self.a2a_prefix.as_str()
    }

    pub fn a2a_prefix_ref(&self) -> &A2aPrefix {
        &self.a2a_prefix
    }

    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }

    pub fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    pub fn task_timeout(&self) -> Duration {
        self.task_timeout
    }

    pub fn max_concurrent_client_tasks(&self) -> usize {
        self.max_concurrent_client_tasks
    }

    pub fn push_dlq_caller_segment(&self) -> &CallerId {
        &self.push_dlq_caller_segment
    }

    #[cfg(test)]
    pub(crate) fn for_test(a2a_prefix: &str) -> Self {
        let nats = NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        };
        Self::new(A2aPrefix::new(a2a_prefix.to_string()).unwrap(), nats)
            .with_task_timeout(crate::constants::TEST_TASK_TIMEOUT)
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
                warn!("{ENV_OPERATION_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default");
            }
            Err(_) => {
                warn!("{ENV_OPERATION_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
            }
        }
    }

    if let Ok(raw) = env_provider.var(ENV_TASK_TIMEOUT_SECS) {
        match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => {
                config = config.with_task_timeout(Duration::from_secs(secs));
            }
            Ok(secs) => {
                warn!("{ENV_TASK_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default");
            }
            Err(_) => {
                warn!("{ENV_TASK_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
            }
        }
    }

    if let Ok(raw) = env_provider.var(ENV_MAX_CONCURRENT_CLIENT_TASKS) {
        match raw.parse::<usize>() {
            Ok(max) => {
                config = config.with_max_concurrent_client_tasks(max);
            }
            Err(_) => {
                warn!(
                    "{ENV_MAX_CONCURRENT_CLIENT_TASKS}={raw:?} is not a valid non-negative integer, using prior value"
                );
            }
        }
    }

    if let Ok(raw) = env_provider.var(ENV_PUSH_DLQ_CALLER_SEGMENT) {
        config = config.with_push_dlq_caller_segment(raw);
    }

    config
}

pub fn nats_connect_timeout<E: ReadEnv>(env_provider: &E) -> Duration {
    let default = Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS);

    match env_provider.var(ENV_CONNECT_TIMEOUT_SECS) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => Duration::from_secs(secs),
            Ok(secs) => {
                warn!("{ENV_CONNECT_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default");
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
    use super::*;
    use crate::constants::{
        DEFAULT_PUSH_DLQ_CALLER_SEGMENT as PLACEHOLDER_CALLER_SEGMENT, ENV_MAX_CONCURRENT_CLIENT_TASKS,
        ENV_PUSH_DLQ_CALLER_SEGMENT,
    };

    fn default_nats() -> NatsConfig {
        NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        }
    }

    fn with_subscriber<F: FnOnce()>(f: F) {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = tracing_subscriber::fmt().with_test_writer().set_default();
        f();
    }

    #[test]
    fn config_new_accepts_validated_prefix() {
        let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats());
        assert_eq!(config.a2a_prefix(), "a2a");
    }

    #[test]
    fn config_with_operation_timeout() {
        let config =
            Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_operation_timeout(Duration::from_secs(45));
        assert_eq!(config.operation_timeout(), Duration::from_secs(45));
    }

    #[test]
    fn config_with_task_timeout() {
        let config =
            Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_task_timeout(Duration::from_secs(60));
        assert_eq!(config.task_timeout(), Duration::from_secs(60));
    }

    #[test]
    fn config_with_max_concurrent_client_tasks_enforces_minimum() {
        let config = Config::for_test("a2a").with_max_concurrent_client_tasks(0);
        assert_eq!(config.max_concurrent_client_tasks(), 1);
    }

    #[test]
    fn config_default_max_concurrent_client_tasks() {
        let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats());
        assert_eq!(
            config.max_concurrent_client_tasks(),
            DEFAULT_MAX_CONCURRENT_CLIENT_TASKS
        );
    }

    #[test]
    fn config_push_dlq_caller_segment_trim_empty_falls_back() {
        let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_push_dlq_caller_segment("");
        assert_eq!(config.push_dlq_caller_segment().as_str(), PLACEHOLDER_CALLER_SEGMENT);
    }

    #[test]
    fn config_push_dlq_caller_segment_preserves_explicit_value() {
        let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_push_dlq_caller_segment("alice");
        assert_eq!(config.push_dlq_caller_segment().as_str(), "alice");
    }

    #[test]
    fn config_nats_returns_nats_config() {
        let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats());
        assert_eq!(config.nats().servers.len(), 1);
    }

    #[test]
    fn nats_connect_timeout_defaults() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );
        });
    }

    #[test]
    fn nats_connect_timeout_valid_override() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            env.set(ENV_CONNECT_TIMEOUT_SECS, "15");
            assert_eq!(nats_connect_timeout(&env), Duration::from_secs(15));
        });
    }

    #[test]
    fn nats_connect_timeout_below_minimum_uses_default() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            env.set(ENV_CONNECT_TIMEOUT_SECS, "0");
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );
        });
    }

    #[test]
    fn nats_connect_timeout_invalid_uses_default() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            env.set(ENV_CONNECT_TIMEOUT_SECS, "bogus");
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );
        });
    }

    #[test]
    fn operation_timeout_invalid_env_is_ignored() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            let default_timeout = Config::for_test("a2a").operation_timeout();
            env.set(ENV_OPERATION_TIMEOUT_SECS, "bogus");
            let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
            assert_eq!(cfg.operation_timeout(), default_timeout);
        });
    }

    #[test]
    fn operation_timeout_below_min_is_ignored() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            let default_timeout = Config::for_test("a2a").operation_timeout();
            env.set(ENV_OPERATION_TIMEOUT_SECS, "0");
            let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
            assert_eq!(cfg.operation_timeout(), default_timeout);
        });
    }

    #[test]
    fn operation_timeout_valid_override() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            env.set(ENV_OPERATION_TIMEOUT_SECS, "45");
            let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
            assert_eq!(cfg.operation_timeout(), Duration::from_secs(45));
        });
    }

    #[test]
    fn task_timeout_invalid_env_is_ignored() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            let default_timeout = Config::for_test("a2a").task_timeout();
            env.set(ENV_TASK_TIMEOUT_SECS, "bogus");
            let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
            assert_eq!(cfg.task_timeout(), default_timeout);
        });
    }

    #[test]
    fn task_timeout_below_min_is_ignored() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            let default_timeout = Config::for_test("a2a").task_timeout();
            env.set(ENV_TASK_TIMEOUT_SECS, "0");
            let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
            assert_eq!(cfg.task_timeout(), default_timeout);
        });
    }

    #[test]
    fn task_timeout_valid_override() {
        with_subscriber(|| {
            let env = trogon_std::env::InMemoryEnv::new();
            env.set(ENV_TASK_TIMEOUT_SECS, "120");
            let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
            assert_eq!(cfg.task_timeout(), Duration::from_secs(120));
        });
    }

    #[test]
    fn push_dlq_caller_segment_env_override() {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_PUSH_DLQ_CALLER_SEGMENT, "oidc-sub-7");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.push_dlq_caller_segment().as_str(), "oidc-sub-7");
    }

    #[test]
    fn push_dlq_caller_segment_blank_env_falls_back_via_builder() {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_PUSH_DLQ_CALLER_SEGMENT, "   ");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.push_dlq_caller_segment().as_str(), PLACEHOLDER_CALLER_SEGMENT);
    }

    #[test]
    fn max_concurrent_client_tasks_env_override() {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_MAX_CONCURRENT_CLIENT_TASKS, "32");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.max_concurrent_client_tasks(), 32);
    }

    #[test]
    fn max_concurrent_client_tasks_env_zero_normalized_to_one() {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_MAX_CONCURRENT_CLIENT_TASKS, "0");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.max_concurrent_client_tasks(), 1);
    }

    #[test]
    fn max_concurrent_client_tasks_invalid_env_is_ignored() {
        let env = trogon_std::env::InMemoryEnv::new();
        let default_max = Config::for_test("a2a").max_concurrent_client_tasks();
        env.set(ENV_MAX_CONCURRENT_CLIENT_TASKS, "bogus");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.max_concurrent_client_tasks(), default_max);
    }
}
