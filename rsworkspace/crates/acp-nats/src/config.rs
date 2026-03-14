use std::time::Duration;
use trogon_nats::NatsConfig;

use crate::acp_prefix::AcpPrefix;

const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_secs(7200);
const DEFAULT_MAX_CONCURRENT_CLIENT_TASKS: usize = 256;

/// Above this value, prompt timeout errors are rendered in seconds instead of milliseconds.
pub(crate) const PROMPT_TIMEOUT_MESSAGE_SECS_THRESHOLD: Duration = Duration::from_secs(60);
/// Suppresses duplicate timeout-related warnings for a short late-response window.
pub(crate) const PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW: Duration = Duration::from_secs(5);
/// Delay before publishing `session.ready` after successful `new_session`/`load_session`.
///
/// The transport returns handler values before the response is fully written to the client.
/// This safety margin preserves expected client-observed ordering.
pub(crate) const SESSION_READY_DELAY: Duration = Duration::from_millis(100);

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
}
