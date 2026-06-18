use std::time::Duration;

use tracing::warn;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_CONNECT_TIMEOUT_SECS, DEFAULT_MCP_PREFIX, DEFAULT_OPERATION_TIMEOUT, ENV_CONNECT_TIMEOUT_SECS,
    ENV_MCP_PREFIX, ENV_OPERATION_TIMEOUT_SECS, MIN_TIMEOUT_SECS,
};
use crate::mcp_prefix::McpPrefix;

pub use crate::constants::{DEFAULT_MCP_PREFIX as DEFAULT_PREFIX, ENV_MCP_PREFIX as ENV_PREFIX};

#[derive(Clone)]
pub struct Config {
    prefix: McpPrefix,
    nats: NatsConfig,
    operation_timeout: Duration,
}

impl Config {
    pub fn new(prefix: McpPrefix, nats: NatsConfig) -> Self {
        Self {
            prefix,
            nats,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
        }
    }

    pub fn from_env<E: ReadEnv>(env_provider: &E) -> Result<Self, crate::McpPrefixError> {
        let raw_prefix = env_provider
            .var(ENV_MCP_PREFIX)
            .unwrap_or_else(|_| DEFAULT_MCP_PREFIX.to_string());
        Ok(Self::new(
            McpPrefix::new(raw_prefix)?,
            NatsConfig::from_env(env_provider),
        ))
    }

    pub fn with_operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout.max(minimum_operation_timeout());
        self
    }

    pub fn prefix(&self) -> &McpPrefix {
        &self.prefix
    }

    pub fn prefix_str(&self) -> &str {
        self.prefix.as_str()
    }

    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }

    pub fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }
}

fn minimum_operation_timeout() -> Duration {
    Duration::from_secs(MIN_TIMEOUT_SECS)
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
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn config_defaults_to_mcp_prefix() {
        let env = InMemoryEnv::new();
        let config = Config::from_env(&env).unwrap();
        assert_eq!(config.prefix_str(), "mcp");
        assert_eq!(config.nats().servers, vec!["localhost:4222"]);
    }

    #[test]
    fn config_reads_prefix_and_nats_from_env() {
        let env = InMemoryEnv::new();
        env.set("MCP_PREFIX", "tenant.mcp");
        env.set("NATS_URL", "nats:4222");
        let config = Config::from_env(&env).unwrap();
        assert_eq!(config.prefix_str(), "tenant.mcp");
        assert_eq!(config.nats().servers, vec!["nats:4222"]);
    }

    #[test]
    fn operation_timeout_override_respects_minimum() {
        let env = InMemoryEnv::new();
        env.set("MCP_OPERATION_TIMEOUT_SECS", "60");
        let config = apply_timeout_overrides(Config::from_env(&env).unwrap(), &env);
        assert_eq!(config.operation_timeout(), Duration::from_secs(60));

        env.set("MCP_OPERATION_TIMEOUT_SECS", "0");
        let config = apply_timeout_overrides(Config::from_env(&env).unwrap(), &env);
        assert_eq!(config.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
    }

    #[test]
    fn operation_timeout_builder_respects_minimum() {
        let env = InMemoryEnv::new();
        let config = Config::from_env(&env).unwrap().with_operation_timeout(Duration::ZERO);
        assert_eq!(config.operation_timeout(), minimum_operation_timeout());

        let config = Config::from_env(&env)
            .unwrap()
            .with_operation_timeout(Duration::from_millis(1));
        assert_eq!(config.operation_timeout(), minimum_operation_timeout());
    }

    #[test]
    fn operation_timeout_override_ignores_invalid_integer() {
        let env = InMemoryEnv::new();
        env.set("MCP_OPERATION_TIMEOUT_SECS", "slow");

        let config = apply_timeout_overrides(Config::from_env(&env).unwrap(), &env);

        assert_eq!(config.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
    }

    #[test]
    fn connect_timeout_override_respects_minimum() {
        let env = InMemoryEnv::new();
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(10));
        env.set("MCP_NATS_CONNECT_TIMEOUT_SECS", "15");
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(15));
        env.set("MCP_NATS_CONNECT_TIMEOUT_SECS", "0");
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(10));
    }

    #[test]
    fn connect_timeout_override_ignores_invalid_integer() {
        let env = InMemoryEnv::new();
        env.set("MCP_NATS_CONNECT_TIMEOUT_SECS", "slow");

        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(10));
    }
}
