use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

const DEFAULT_PORT: u16 = 8080;
const DEFAULT_SUBJECT_PREFIX: &str = "github";
const DEFAULT_STREAM_NAME: &str = "GITHUB";

/// Configuration for the GitHub webhook server.
///
/// Resolved from environment variables:
/// - `GITHUB_WEBHOOK_SECRET`: HMAC-SHA256 secret configured in GitHub (required for signature validation)
/// - `GITHUB_WEBHOOK_PORT`: HTTP listening port (default: 8080)
/// - `GITHUB_SUBJECT_PREFIX`: NATS subject prefix (default: `github`)
/// - `GITHUB_STREAM_NAME`: JetStream stream name (default: `GITHUB`)
/// - Standard `NATS_*` variables for NATS connection (see `trogon-nats`)
pub struct GithubConfig {
    pub webhook_secret: Option<String>,
    pub port: u16,
    pub subject_prefix: String,
    pub stream_name: String,
    pub nats: NatsConfig,
}

impl GithubConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            webhook_secret: env.var("GITHUB_WEBHOOK_SECRET").ok(),
            port: env
                .var("GITHUB_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: env
                .var("GITHUB_SUBJECT_PREFIX")
                .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            stream_name: env
                .var("GITHUB_STREAM_NAME")
                .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
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
        let config = GithubConfig::from_env(&env);

        assert!(config.webhook_secret.is_none());
        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix, "github");
        assert_eq!(config.stream_name, "GITHUB");
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("GITHUB_WEBHOOK_SECRET", "my-secret");
        env.set("GITHUB_WEBHOOK_PORT", "9090");
        env.set("GITHUB_SUBJECT_PREFIX", "gh");
        env.set("GITHUB_STREAM_NAME", "GH_EVENTS");

        let config = GithubConfig::from_env(&env);

        assert_eq!(config.webhook_secret.as_deref(), Some("my-secret"));
        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix, "gh");
        assert_eq!(config.stream_name, "GH_EVENTS");
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("GITHUB_WEBHOOK_PORT", "not-a-number");

        let config = GithubConfig::from_env(&env);

        assert_eq!(config.port, 8080);
    }
}
