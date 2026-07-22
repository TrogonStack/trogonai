//! Env-driven [`Args`] + resolved [`Config`] for the gateway.
//!
//! The CLI surface is intentionally small — gateway tuning lives on `NatsConfig`
//! and per-slice env vars (queue group, JWT audience, trust-caller-headers).
//! `config_from_args` is the single seam tests and the binary share so the
//! same env-resolution path is exercised under both.

use std::fmt;

use a2a_nats::{A2aPrefix, A2aPrefixError, NatsConfig};
use clap::Parser;
use trogon_std::env::ReadEnv;

const ENV_GATEWAY_QUEUE_GROUP: &str = "A2A_GATEWAY_QUEUE_GROUP";

/// Validated NATS server URL. Carrying the validation in the type stops
/// later layers (connect, audit) from inheriting empty/whitespace
/// primitives that look usable until the first network call fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatsServerUrl(String);

impl NatsServerUrl {
    pub fn new(raw: impl Into<String>) -> Result<Self, NatsServerUrlError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(NatsServerUrlError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NatsServerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NatsServerUrlError {
    #[error("NATS server URL must not be empty")]
    Empty,
}

/// Validated NATS queue group name. Non-empty, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueGroup(String);

impl QueueGroup {
    pub fn new(raw: impl Into<String>) -> Result<Self, QueueGroupError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(QueueGroupError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QueueGroupError {
    #[error("queue group must not be empty")]
    Empty,
}

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
/// `Config` carries gateway-specific values. Every field is a validated
/// value object so later layers can't inherit invalid primitives.
#[derive(Debug, Clone)]
pub struct Config {
    pub nats_servers: Vec<NatsServerUrl>,
    pub a2a_prefix: A2aPrefix,
    pub queue_group: Option<QueueGroup>,
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
    /// `--nats-url` / `NATS_URL` parsed to zero usable servers. Refuse
    /// rather than return a config that defers the failure to the first
    /// connect attempt.
    #[error("--nats-url / NATS_URL must list at least one server, got {raw:?}")]
    EmptyNatsServers { raw: String },
    /// One of the comma-separated `--nats-url` entries did not validate as
    /// a NATS server URL.
    #[error("invalid NATS server URL")]
    InvalidNatsServerUrl(#[source] NatsServerUrlError),
    /// `--queue-group` / `A2A_GATEWAY_QUEUE_GROUP` was present but empty
    /// after trimming.
    #[error("invalid queue group")]
    InvalidQueueGroup(#[source] QueueGroupError),
}

pub fn config_from_args<E: ReadEnv>(args: Args, env: &E) -> Result<(Config, NatsConfig), ConfigError> {
    let a2a_prefix = A2aPrefix::new(args.prefix).map_err(ConfigError::InvalidPrefix)?;

    let nats_servers = parse_servers(&args.nats_url)?;
    let mut nats_config = NatsConfig::from_env(env);
    nats_config.servers = nats_servers.iter().map(|s| s.as_str().to_owned()).collect();

    // Resolve queue group from CLI first, then env. Validate the chosen
    // value through QueueGroup so an empty env value is rejected the same
    // way an empty CLI value would be.
    let queue_group_raw = args.queue_group.or_else(|| env.var(ENV_GATEWAY_QUEUE_GROUP).ok());
    let queue_group = match queue_group_raw {
        Some(raw) => Some(QueueGroup::new(raw).map_err(ConfigError::InvalidQueueGroup)?),
        None => None,
    };

    let config = Config {
        nats_servers,
        a2a_prefix,
        queue_group,
    };

    Ok((config, nats_config))
}

fn parse_servers(raw: &str) -> Result<Vec<NatsServerUrl>, ConfigError> {
    let servers: Vec<NatsServerUrl> = raw
        .split(',')
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .map(NatsServerUrl::new)
        .collect::<Result<_, _>>()
        .map_err(ConfigError::InvalidNatsServerUrl)?;
    if servers.is_empty() {
        return Err(ConfigError::EmptyNatsServers { raw: raw.to_owned() });
    }
    Ok(servers)
}

#[cfg(test)]
mod tests;
