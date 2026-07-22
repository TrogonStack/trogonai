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
mod tests;
