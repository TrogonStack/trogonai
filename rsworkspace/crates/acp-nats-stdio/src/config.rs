use acp_nats::{AcpPrefix, AcpPrefixError, Config, NatsConfig};
use clap::Parser;
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

#[derive(Parser, Debug, Clone)]
#[command(name = "acp-nats-stdio")]
#[command(about = "ACP stdio to NATS bridge for agent-client protocol", long_about = None)]
pub struct Args {
    #[arg(long = "acp-prefix")]
    pub acp_prefix: Option<String>,
}

pub fn base_config<P: ParseArgs<Args = Args>, E: ReadEnv>(
    parser: &P,
    env_provider: &E,
) -> Result<Config, AcpPrefixError> {
    let args = parser.parse_args();
    base_config_from_args(args, env_provider)
}

fn base_config_from_args<E: ReadEnv>(args: Args, env_provider: &E) -> Result<Config, AcpPrefixError> {
    let raw_prefix = args
        .acp_prefix
        .or_else(|| env_provider.var(acp_nats::ENV_ACP_PREFIX).ok())
        .unwrap_or_else(|| acp_nats::DEFAULT_ACP_PREFIX.to_string());
    let prefix = AcpPrefix::new(raw_prefix)?;
    Ok(Config::with_prefix(prefix, NatsConfig::from_env(env_provider)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::FixedArgs;
    use trogon_std::env::InMemoryEnv;

    fn config_from_env(env: &InMemoryEnv) -> Config {
        let parser = FixedArgs(Args { acp_prefix: None });
        let config = base_config(&parser, env).unwrap();
        acp_nats::apply_timeout_overrides(config, env)
    }

    #[test]
    fn test_default_config() {
        let env = InMemoryEnv::new();
        let config = config_from_env(&env);
        assert_eq!(config.acp_prefix(), acp_nats::DEFAULT_ACP_PREFIX);
        assert_eq!(config.nats().servers, vec!["localhost:4222"]);
        assert!(matches!(&config.nats().auth, acp_nats::NatsAuth::None));
    }

    #[test]
    fn test_acp_prefix_from_env_provider() {
        let env = InMemoryEnv::new();
        env.set("ACP_PREFIX", "custom-prefix");
        let config = config_from_env(&env);
        assert_eq!(config.acp_prefix(), "custom-prefix");
    }

    #[test]
    fn test_acp_prefix_from_args() {
        let env = InMemoryEnv::new();
        let parser = FixedArgs(Args {
            acp_prefix: Some("cli-prefix".to_string()),
        });
        let config = base_config(&parser, &env).unwrap();
        assert_eq!(config.acp_prefix(), "cli-prefix");
    }

    #[test]
    fn test_args_override_env() {
        let env = InMemoryEnv::new();
        env.set("ACP_PREFIX", "env-prefix");
        let parser = FixedArgs(Args {
            acp_prefix: Some("cli-prefix".to_string()),
        });
        let config = base_config(&parser, &env).unwrap();
        assert_eq!(config.acp_prefix(), "cli-prefix");
    }

    #[test]
    fn test_nats_config_from_env() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222,host2:4222");
        env.set("NATS_TOKEN", "my-token");
        let config = config_from_env(&env);
        assert_eq!(config.nats().servers, vec!["host1:4222", "host2:4222"]);
        assert!(matches!(&config.nats().auth, acp_nats::NatsAuth::Token(t) if t == "my-token"));
    }

    #[test]
    fn test_base_config_via_parse_args_trait() {
        let env = InMemoryEnv::new();
        let parser = FixedArgs(Args {
            acp_prefix: Some("trait-test".to_string()),
        });
        let config = base_config(&parser, &env).unwrap();
        assert_eq!(config.acp_prefix(), "trait-test");
    }
}
