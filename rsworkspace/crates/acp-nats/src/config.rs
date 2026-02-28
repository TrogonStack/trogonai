use std::time::Duration;
use trogon_nats::NatsConfig;

use crate::acp_prefix::AcpPrefix;

const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_secs(7200);

#[derive(Clone)]
pub struct Config {
    pub(crate) acp_prefix: AcpPrefix,
    pub(crate) nats: NatsConfig,
    pub(crate) operation_timeout: Duration,
    pub(crate) prompt_timeout: Duration,
}

impl Config {
    pub fn new(acp_prefix: AcpPrefix, nats: NatsConfig) -> Self {
        Self {
            acp_prefix,
            nats,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            prompt_timeout: DEFAULT_PROMPT_TIMEOUT,
        }
    }

    pub fn with_operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout;
        self
    }

    pub fn with_prompt_timeout(mut self, timeout: Duration) -> Self {
        self.prompt_timeout = timeout;
        self
    }

    pub fn acp_prefix(&self) -> &str {
        self.acp_prefix.as_str()
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

    #[cfg(test)]
    pub(crate) fn for_test(acp_prefix: &str) -> Self {
        let nats = NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        };
        Self::new(AcpPrefix::new(acp_prefix).unwrap(), nats)
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
    fn config_nats_returns_nats_config() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
        assert_eq!(config.nats().servers.len(), 1);
        assert_eq!(config.nats().servers[0], "localhost:4222");
    }
}
