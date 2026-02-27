use acp_nats::{Config, NatsConfig};
use clap::Parser;
use std::time::Duration;
use tracing::warn;
use trogon_std::env::ReadEnv;

const MIN_TIMEOUT_SECS: u64 = 1;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

const ENV_ACP_PREFIX: &str = "ACP_PREFIX";
const ENV_OPERATION_TIMEOUT_SECS: &str = "ACP_OPERATION_TIMEOUT_SECS";
const ENV_PROMPT_TIMEOUT_SECS: &str = "ACP_PROMPT_TIMEOUT_SECS";
const ENV_CONNECT_TIMEOUT_SECS: &str = "ACP_NATS_CONNECT_TIMEOUT_SECS";
const DEFAULT_ACP_PREFIX: &str = "acp";

#[derive(Parser, Debug)]
#[command(name = "acp-nats-stdio")]
#[command(about = "ACP stdio to NATS bridge for agent-client protocol", long_about = None)]
pub struct Args {
    #[arg(long = "acp-prefix")]
    pub acp_prefix: Option<String>,
}

pub fn from_env_with_provider<E: ReadEnv>(
    env_provider: &E,
) -> Result<Config, acp_nats::ValidationError> {
    let args = Args::parse();
    from_args(args, env_provider)
}

pub fn from_args<E: ReadEnv>(
    args: Args,
    env_provider: &E,
) -> Result<Config, acp_nats::ValidationError> {
    let acp_prefix = args
        .acp_prefix
        .or_else(|| env_provider.var(ENV_ACP_PREFIX).ok())
        .unwrap_or_else(|| DEFAULT_ACP_PREFIX.to_string());
    let mut config = Config::new(acp_prefix, NatsConfig::from_env(env_provider))?;

    if let Ok(raw) = env_provider.var(ENV_OPERATION_TIMEOUT_SECS) {
        match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => {
                config = config.with_operation_timeout(Duration::from_secs(secs));
            }
            Ok(secs) => {
                warn!(
                    "{ENV_OPERATION_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
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
                warn!(
                    "{ENV_PROMPT_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
            }
            Err(_) => {
                warn!("{ENV_PROMPT_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
            }
        }
    }

    Ok(config)
}

pub fn nats_connect_timeout<E: ReadEnv>(env_provider: &E) -> Duration {
    let default = Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS);

    match env_provider.var(ENV_CONNECT_TIMEOUT_SECS) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => Duration::from_secs(secs),
            Ok(secs) => {
                warn!(
                    "{ENV_CONNECT_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
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
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    fn config_from_env(env: &InMemoryEnv) -> Config {
        let args = Args { acp_prefix: None };
        from_args(args, env).unwrap()
    }

    #[test]
    fn test_default_config() {
        let env = InMemoryEnv::new();
        let config = config_from_env(&env);
        assert_eq!(config.acp_prefix(), DEFAULT_ACP_PREFIX);
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
        let args = Args {
            acp_prefix: Some("cli-prefix".to_string()),
        };
        let config = from_args(args, &env).unwrap();
        assert_eq!(config.acp_prefix(), "cli-prefix");
    }

    #[test]
    fn test_args_override_env() {
        let env = InMemoryEnv::new();
        env.set("ACP_PREFIX", "env-prefix");
        let args = Args {
            acp_prefix: Some("cli-prefix".to_string()),
        };
        let config = from_args(args, &env).unwrap();
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
    fn test_nats_connect_timeout_from_env() {
        let env = InMemoryEnv::new();
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS));

        env.set(ENV_CONNECT_TIMEOUT_SECS, "15");
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(15));
        env.set(ENV_CONNECT_TIMEOUT_SECS, "0");
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS));
        env.set(ENV_CONNECT_TIMEOUT_SECS, "not-a-number");
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS));
    }

    #[test]
    fn test_operation_timeout_invalid_env_is_ignored() {
        let env = InMemoryEnv::new();
        let default = config_from_env(&env);
        let default_timeout = default.operation_timeout();

        env.set("ACP_OPERATION_TIMEOUT_SECS", "not-a-number");
        let invalid = config_from_env(&env);
        assert_eq!(invalid.operation_timeout(), default_timeout);
    }

    #[test]
    fn test_operation_timeout_under_min_is_ignored() {
        let env = InMemoryEnv::new();
        let default = config_from_env(&env);
        let default_timeout = default.operation_timeout();

        env.set("ACP_OPERATION_TIMEOUT_SECS", "0");
        let ignored = config_from_env(&env);
        assert_eq!(ignored.operation_timeout(), default_timeout);
    }

    #[test]
    fn test_prompt_timeout_invalid_env_is_ignored() {
        let env = InMemoryEnv::new();
        let default = config_from_env(&env);
        let default_timeout = default.prompt_timeout();

        env.set("ACP_PROMPT_TIMEOUT_SECS", "not-a-number");
        let invalid = config_from_env(&env);
        assert_eq!(invalid.prompt_timeout(), default_timeout);
    }

    #[test]
    fn test_prompt_timeout_under_min_is_ignored() {
        let env = InMemoryEnv::new();
        let default = config_from_env(&env);
        let default_timeout = default.prompt_timeout();

        env.set("ACP_PROMPT_TIMEOUT_SECS", "0");
        let ignored = config_from_env(&env);
        assert_eq!(ignored.prompt_timeout(), default_timeout);
    }
}
