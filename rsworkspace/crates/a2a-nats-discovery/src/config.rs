use std::fmt;

use a2a_nats::{A2aPrefix, A2aPrefixError, NatsConfig};
use clap::Parser;
use trogon_std::env::ReadEnv;

#[derive(Parser, Debug)]
#[command(name = "a2a-nats-discovery")]
#[command(about = "A2A agent catalog discovery service over NATS", long_about = None)]
pub struct Args {
    /// Comma-separated NATS server URL(s). Overrides `NATS_URL` when passed explicitly.
    #[arg(long, env = "NATS_URL", default_value = "localhost:4222")]
    pub nats_url: String,

    /// A2A subject prefix for discover request/reply subjects (`{prefix}.discover.*`).
    #[arg(long, env = "A2A_PREFIX", default_value = "a2a")]
    pub prefix: String,
}

#[derive(Debug)]
pub enum ConfigError {
    InvalidPrefix(A2aPrefixError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrefix(error) => write!(f, "invalid A2A prefix: {error}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidPrefix(error) => Some(error),
        }
    }
}

pub fn config_from_args<E: ReadEnv>(args: Args, env: &E) -> Result<(A2aPrefix, NatsConfig), ConfigError> {
    let prefix = A2aPrefix::new(args.prefix).map_err(ConfigError::InvalidPrefix)?;

    let mut nats_config = NatsConfig::from_env(env);
    nats_config.servers = parse_servers(&args.nats_url);

    Ok((prefix, nats_config))
}

fn parse_servers(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn config_from_args_validates_prefix() {
        let env = InMemoryEnv::new();
        let args = Args {
            nats_url: "localhost:4222".to_string(),
            prefix: "bad prefix!".to_string(),
        };

        let result = config_from_args(args, &env);

        assert!(matches!(result, Err(ConfigError::InvalidPrefix(_))));
    }

    #[test]
    fn config_from_args_parses_comma_separated_servers() {
        let env = InMemoryEnv::new();
        let args = Args {
            nats_url: "host1:4222 , host2:4222".to_string(),
            prefix: "a2a".to_string(),
        };

        let (_, nats_config) = config_from_args(args, &env).unwrap();

        assert_eq!(nats_config.servers, vec!["host1:4222", "host2:4222"]);
    }

    #[test]
    fn config_from_args_uses_env_auth() {
        let env = InMemoryEnv::new();
        env.set("NATS_TOKEN", "secret");
        let args = Args {
            nats_url: "localhost:4222".to_string(),
            prefix: "a2a".to_string(),
        };

        let (_, nats_config) = config_from_args(args, &env).unwrap();

        assert!(matches!(nats_config.auth, a2a_nats::NatsAuth::Token(token) if token == "secret"));
    }
}
