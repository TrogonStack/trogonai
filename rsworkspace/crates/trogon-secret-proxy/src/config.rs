//! Configuration for the secret proxy and its worker.

use std::time::Duration;
use trogon_nats::NatsConfig;

const DEFAULT_PROXY_PORT: u16 = 8080;
const DEFAULT_WORKER_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const DEFAULT_SUBJECT_PREFIX: &str = "trogon";

/// Configuration shared between the HTTP proxy server and the JetStream worker.
#[derive(Debug, Clone)]
pub struct Config {
    /// NATS subject prefix (e.g. `"trogon"`).
    pub(crate) prefix: String,
    /// NATS connection configuration.
    pub(crate) nats: NatsConfig,
    /// TCP port the HTTP proxy listens on. Default: `8080`.
    pub(crate) proxy_port: u16,
    /// How long the proxy waits for the worker to reply. Default: `60s`.
    pub(crate) worker_timeout: Duration,
}

impl Config {
    /// Create a new config with the given prefix and NATS connection config.
    pub fn new(prefix: impl Into<String>, nats: NatsConfig) -> Self {
        Self {
            prefix: prefix.into(),
            nats,
            proxy_port: DEFAULT_PROXY_PORT,
            worker_timeout: DEFAULT_WORKER_TIMEOUT,
        }
    }

    /// Override the TCP port the proxy listens on.
    pub fn with_proxy_port(mut self, port: u16) -> Self {
        self.proxy_port = port;
        self
    }

    /// Override the timeout the proxy waits for a worker reply.
    pub fn with_worker_timeout(mut self, timeout: Duration) -> Self {
        self.worker_timeout = timeout;
        self
    }

    /// Subject prefix (e.g. `"trogon"`).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// NATS connection config.
    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }

    /// Proxy listen port.
    pub fn proxy_port(&self) -> u16 {
        self.proxy_port
    }

    /// Worker reply timeout.
    pub fn worker_timeout(&self) -> Duration {
        self.worker_timeout
    }

    /// Convenience constructor for tests using a local NATS server.
    #[cfg(test)]
    pub fn for_test() -> Self {
        let nats = NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        };
        Self::new(DEFAULT_SUBJECT_PREFIX, nats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let cfg = Config::for_test();
        assert_eq!(cfg.proxy_port(), DEFAULT_PROXY_PORT);
        assert_eq!(cfg.worker_timeout(), DEFAULT_WORKER_TIMEOUT);
        assert_eq!(cfg.prefix(), DEFAULT_SUBJECT_PREFIX);
    }

    #[test]
    fn with_proxy_port() {
        let cfg = Config::for_test().with_proxy_port(9090);
        assert_eq!(cfg.proxy_port(), 9090);
    }

    #[test]
    fn with_worker_timeout() {
        let cfg = Config::for_test().with_worker_timeout(Duration::from_secs(30));
        assert_eq!(cfg.worker_timeout(), Duration::from_secs(30));
    }
}
