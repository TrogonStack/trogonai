use std::time::Duration;
use tracing::warn;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::acp_prefix::AcpPrefix;
use crate::constants::{
    DEFAULT_CONNECT_TIMEOUT_SECS, DEFAULT_MAX_CONCURRENT_CLIENT_TASKS, DEFAULT_OPERATION_TIMEOUT,
    DEFAULT_PROMPT_TIMEOUT, ENV_CONNECT_TIMEOUT_SECS, ENV_OPERATION_TIMEOUT_SECS, ENV_PROMPT_TIMEOUT_SECS,
    MIN_TIMEOUT_SECS,
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
                warn!("{ENV_OPERATION_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default");
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
                warn!("{ENV_PROMPT_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default");
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
