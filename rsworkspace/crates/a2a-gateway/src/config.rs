//! Env-driven [`Args`] + resolved [`Config`] for the gateway.
//!
//! The CLI surface is intentionally small — gateway tuning lives on `NatsConfig`
//! and per-slice env vars (queue group, JWT audience, trust-caller-headers).
//! `config_from_args` is the single seam tests and the binary share so the
//! same env-resolution path is exercised under both.

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

/// Resolved gateway configuration. Pairs with [`NatsConfig`] returned from
/// [`config_from_args`] — `NatsConfig` carries the wire-level NATS knobs and
/// `Config` carries gateway-specific values.
#[derive(Debug, Clone)]
pub struct Config {
    pub nats_servers: Vec<String>,
    pub a2a_prefix: A2aPrefix,
    pub queue_group: Option<String>,
}

impl Config {
    /// Subject the gateway subscribes to. Always `{prefix}.gateway.>` so a
    /// single subscriber serves every agent / method behind the gateway.
    pub fn gateway_subscribe_subject(&self) -> String {
        format!("{}.gateway.>", self.a2a_prefix)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// `--prefix` / `A2A_PREFIX` failed the subject-segment validation in
    /// `a2a-nats`. Surface the typed source so operators see which character
    /// is rejected.
    #[error("invalid A2A prefix")]
    InvalidPrefix(#[source] A2aPrefixError),
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
mod tests;
