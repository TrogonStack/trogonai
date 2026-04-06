use trogon_std::env::ReadEnv;

const ENV_TELEGRAM_PREFIX: &str = "TELEGRAM_PREFIX";
const DEFAULT_PREFIX: &str = "prod";

#[derive(Debug, Clone)]
pub struct TelegramNatsConfig {
    pub nats: trogon_nats::NatsConfig,
    pub prefix: String,
}

impl TelegramNatsConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let prefix = env
            .var(ENV_TELEGRAM_PREFIX)
            .unwrap_or_else(|_| DEFAULT_PREFIX.to_string());

        Self {
            nats: trogon_nats::NatsConfig::from_env(env),
            prefix,
        }
    }

    pub fn new(nats: trogon_nats::NatsConfig, prefix: impl Into<String>) -> Self {
        Self {
            nats,
            prefix: prefix.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn from_env_defaults() {
        let env = InMemoryEnv::new();
        let cfg = TelegramNatsConfig::from_env(&env);
        assert_eq!(cfg.prefix, "prod");
        assert_eq!(cfg.nats.servers, vec!["localhost:4222"]);
    }

    #[test]
    fn from_env_custom_prefix() {
        let env = InMemoryEnv::new();
        env.set("TELEGRAM_PREFIX", "staging");
        let cfg = TelegramNatsConfig::from_env(&env);
        assert_eq!(cfg.prefix, "staging");
    }

    #[test]
    fn from_env_custom_nats_url() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats1:4222,nats2:4222");
        let cfg = TelegramNatsConfig::from_env(&env);
        assert_eq!(cfg.nats.servers, vec!["nats1:4222", "nats2:4222"]);
    }

    #[test]
    fn new_constructor() {
        let nats = trogon_nats::NatsConfig::from_url("nats://localhost:4222");
        let cfg = TelegramNatsConfig::new(nats, "dev");
        assert_eq!(cfg.prefix, "dev");
    }
}
