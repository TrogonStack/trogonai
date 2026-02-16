use acp_nats::{Config, NatsConfig};
use clap::Parser;
use trogon_std::env::ReadEnv;

const ENV_ACP_PREFIX: &str = "ACP_PREFIX";
const DEFAULT_ACP_PREFIX: &str = "acp";

#[derive(Parser, Debug)]
#[command(name = "acp-nats-stdio")]
#[command(about = "ACP stdio to NATS bridge for agent-client protocol", long_about = None)]
pub struct Args {
    #[arg(long = "acp-prefix", env = ENV_ACP_PREFIX)]
    pub acp_prefix: Option<String>,
}

pub fn from_env_with_provider<E: ReadEnv>(env_provider: &E) -> Config {
    let args = Args::parse();
    Config::new(
        args.acp_prefix
            .unwrap_or_else(|| DEFAULT_ACP_PREFIX.to_string()),
        NatsConfig::from_env(env_provider),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn test_default_config() {
        let env = InMemoryEnv::new();
        let config = from_env_with_provider(&env);
        assert_eq!(config.acp_prefix, DEFAULT_ACP_PREFIX);
        assert_eq!(config.nats.servers, vec!["localhost:4222"]);
        assert!(matches!(config.nats.auth, acp_nats::NatsAuth::None));
    }

    #[test]
    fn test_nats_config_from_env() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222,host2:4222");
        env.set("NATS_TOKEN", "my-token");
        let config = from_env_with_provider(&env);
        assert_eq!(config.nats.servers, vec!["host1:4222", "host2:4222"]);
        assert!(matches!(config.nats.auth, acp_nats::NatsAuth::Token(t) if t == "my-token"));
    }
}
