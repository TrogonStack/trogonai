use std::fmt;

use a2a_nats::{A2aPrefix, A2aPrefixError, NatsConfig};
use clap::Parser;
use trogon_std::env::ReadEnv;

const ENV_GATEWAY_QUEUE_GROUP: &str = "A2A_GATEWAY_QUEUE_GROUP";

#[derive(Parser, Debug)]
#[command(name = "a2a-gateway")]
#[command(about = "A2A gateway — ingress subscription and forward to agent subjects", long_about = None)]
pub struct Args {
    /// Comma-separated NATS server URL(s). Overrides `NATS_URL` when passed explicitly.
    #[arg(long, env = "NATS_URL", default_value = "localhost:4222")]
    pub nats_url: String,

    /// A2A subject prefix for gateway and agent subjects.
    #[arg(long, env = "A2A_PREFIX", default_value = "a2a")]
    pub prefix: String,

    /// Optional NATS queue group for `{prefix}.gateway.>` subscribers.
    #[arg(long, env = "A2A_GATEWAY_QUEUE_GROUP")]
    pub queue_group: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub nats_servers: Vec<String>,
    pub a2a_prefix: A2aPrefix,
    pub queue_group: Option<String>,
}

impl Config {
    pub fn gateway_subscribe_subject(&self) -> String {
        format!("{}.gateway.>", self.a2a_prefix)
    }
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

pub fn config_from_args<E: ReadEnv>(args: Args, env: &E) -> Result<(Config, NatsConfig), ConfigError> {
    let a2a_prefix = A2aPrefix::new(args.prefix).map_err(ConfigError::InvalidPrefix)?;

    let mut nats_config = NatsConfig::from_env(env);
    nats_config.servers = parse_servers(&args.nats_url);

    let queue_group = args.queue_group.or_else(|| env.var(ENV_GATEWAY_QUEUE_GROUP).ok());

    let config = Config {
        nats_servers: nats_config.servers.clone(),
        a2a_prefix,
        queue_group,
    };

    Ok((config, nats_config))
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
            queue_group: None,
        };

        let result = config_from_args(args, &env);

        assert!(matches!(result, Err(ConfigError::InvalidPrefix(_))));
    }

    #[test]
    fn config_gateway_subscribe_subject() {
        let env = InMemoryEnv::new();
        let args = Args {
            nats_url: "localhost:4222".to_string(),
            prefix: "a2a".to_string(),
            queue_group: Some("gateways".to_string()),
        };

        let (config, _) = config_from_args(args, &env).unwrap();

        assert_eq!(config.gateway_subscribe_subject(), "a2a.gateway.>");
        assert_eq!(config.queue_group.as_deref(), Some("gateways"));
    }
}
