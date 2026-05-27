use std::time::Duration;

use trogon_sts::EXCHANGE_SUBJECT;

pub const ENV_STS_EXCHANGE_SUBJECT: &str = "MCP_GATEWAY_STS_EXCHANGE_SUBJECT";
pub const ENV_STS_TIMEOUT_MS: &str = "MCP_GATEWAY_STS_TIMEOUT_MS";

const DEFAULT_TIMEOUT_MS: u64 = 100;

#[derive(Clone, Debug)]
pub struct StsClientConfig {
    pub exchange_subject: String,
    pub timeout: Duration,
}

impl StsClientConfig {
    pub fn from_env<E: trogon_std::env::ReadEnv>(env: &E) -> Self {
        let exchange_subject = env
            .var(ENV_STS_EXCHANGE_SUBJECT)
            .unwrap_or_else(|_| EXCHANGE_SUBJECT.to_string());
        let timeout_ms = env
            .var(ENV_STS_TIMEOUT_MS)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        Self {
            exchange_subject,
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timeout_is_100ms() {
        assert_eq!(DEFAULT_TIMEOUT_MS, 100);
    }
}
