use std::time::Duration;

use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

const DEFAULT_PORT: u16 = 8080;
const DEFAULT_STREAM_NAME: &str = "TRANSCRIPTS";
const DEFAULT_SUBJECT_FILTER: &str = "transcripts.>";
const DEFAULT_CONSUMER_NAME: &str = "webhook-dispatcher";
const DEFAULT_DISPATCH_TIMEOUT_SECS: u64 = 30;

/// Configuration for the webhook dispatcher service.
///
/// | Variable                      | Default              | Description                                      |
/// |-------------------------------|----------------------|--------------------------------------------------|
/// | `WEBHOOK_STREAM_NAME`         | `TRANSCRIPTS`        | JetStream stream to subscribe to                 |
/// | `WEBHOOK_SUBJECT_FILTER`      | `transcripts.>`      | Subject filter for the durable consumer          |
/// | `WEBHOOK_CONSUMER_NAME`       | `webhook-dispatcher` | Durable consumer name                            |
/// | `WEBHOOK_PORT`                | `8080`               | Management HTTP API port                         |
/// | `WEBHOOK_DISPATCH_TIMEOUT_SECS` | `30`               | Per-request HTTP timeout for outbound deliveries |
/// | Standard `NATS_*` variables for NATS connection (see `trogon-nats`)
pub struct WebhookDispatcherConfig {
    pub stream_name: String,
    pub subject_filter: String,
    pub consumer_name: String,
    pub port: u16,
    pub dispatch_timeout: Duration,
    pub nats: NatsConfig,
}

impl WebhookDispatcherConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            stream_name: env
                .var("WEBHOOK_STREAM_NAME")
                .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            subject_filter: env
                .var("WEBHOOK_SUBJECT_FILTER")
                .unwrap_or_else(|_| DEFAULT_SUBJECT_FILTER.to_string()),
            consumer_name: env
                .var("WEBHOOK_CONSUMER_NAME")
                .unwrap_or_else(|_| DEFAULT_CONSUMER_NAME.to_string()),
            port: env
                .var("WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            dispatch_timeout: Duration::from_secs(
                env.var("WEBHOOK_DISPATCH_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_DISPATCH_TIMEOUT_SECS),
            ),
            nats: NatsConfig::from_env(env),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn defaults_when_no_env_vars() {
        let env = InMemoryEnv::new();
        let config = WebhookDispatcherConfig::from_env(&env);

        assert_eq!(config.stream_name, "TRANSCRIPTS");
        assert_eq!(config.subject_filter, "transcripts.>");
        assert_eq!(config.consumer_name, "webhook-dispatcher");
        assert_eq!(config.port, 8080);
        assert_eq!(config.dispatch_timeout, Duration::from_secs(30));
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("WEBHOOK_STREAM_NAME", "GITHUB");
        env.set("WEBHOOK_SUBJECT_FILTER", "github.>");
        env.set("WEBHOOK_CONSUMER_NAME", "my-consumer");
        env.set("WEBHOOK_PORT", "9090");
        env.set("WEBHOOK_DISPATCH_TIMEOUT_SECS", "60");

        let config = WebhookDispatcherConfig::from_env(&env);

        assert_eq!(config.stream_name, "GITHUB");
        assert_eq!(config.subject_filter, "github.>");
        assert_eq!(config.consumer_name, "my-consumer");
        assert_eq!(config.port, 9090);
        assert_eq!(config.dispatch_timeout, Duration::from_secs(60));
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("WEBHOOK_PORT", "not-a-number");
        let config = WebhookDispatcherConfig::from_env(&env);
        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn invalid_timeout_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("WEBHOOK_DISPATCH_TIMEOUT_SECS", "not-a-number");
        let config = WebhookDispatcherConfig::from_env(&env);
        assert_eq!(config.dispatch_timeout, Duration::from_secs(DEFAULT_DISPATCH_TIMEOUT_SECS));
    }
}
