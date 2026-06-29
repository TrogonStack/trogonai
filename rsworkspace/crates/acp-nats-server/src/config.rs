use acp_nats::{AcpPrefix, AcpPrefixError, Config, NatsConfig};
use clap::Parser;
use std::net::IpAddr;
use trogon_std::env::ReadEnv;

use crate::constants::{DEFAULT_HOST, DEFAULT_PORT};

#[derive(Parser, Debug)]
#[command(name = "acp-nats-server")]
#[command(about = "ACP transport server bridging agent-client protocol to NATS", long_about = None)]
pub struct Args {
    #[arg(long = "acp-prefix")]
    pub acp_prefix: Option<String>,

    #[arg(long)]
    pub host: Option<IpAddr>,

    #[arg(long)]
    pub port: Option<u16>,
}

pub struct ServerConfig {
    pub acp: Config,
    pub host: IpAddr,
    pub port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerConfigError {
    #[error("{0}")]
    InvalidAcpPrefix(#[source] AcpPrefixError),
    #[error("invalid value for {key}: {value:?} ({message})")]
    InvalidEnvVar {
        key: &'static str,
        value: String,
        message: String,
    },
}

impl From<AcpPrefixError> for ServerConfigError {
    fn from(error: AcpPrefixError) -> Self {
        Self::InvalidAcpPrefix(error)
    }
}

pub fn config_from_args<E: ReadEnv>(args: Args, env_provider: &E) -> Result<ServerConfig, ServerConfigError> {
    let raw_prefix = args
        .acp_prefix
        .or_else(|| env_provider.var(acp_nats::ENV_ACP_PREFIX).ok())
        .unwrap_or_else(|| acp_nats::DEFAULT_ACP_PREFIX.to_string());
    let prefix = AcpPrefix::new(raw_prefix)?;
    Ok(ServerConfig {
        acp: Config::with_prefix(prefix, NatsConfig::from_env(env_provider)),
        host: args
            .host
            .or(read_env_var(env_provider, "ACP_SERVER_HOST")?)
            .unwrap_or(DEFAULT_HOST),
        port: args
            .port
            .or(read_env_var(env_provider, "ACP_SERVER_PORT")?)
            .unwrap_or(DEFAULT_PORT),
    })
}

fn read_env_var<T, E>(env_provider: &E, key: &'static str) -> Result<Option<T>, ServerConfigError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
    E: ReadEnv,
{
    match env_provider.var(key) {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|error: T::Err| ServerConfigError::InvalidEnvVar {
                key,
                value,
                message: error.to_string(),
            }),
        Err(_) => Ok(None),
    }
}

pub fn apply_timeout_overrides<E: ReadEnv>(mut server: ServerConfig, env_provider: &E) -> ServerConfig {
    server.acp = acp_nats::apply_timeout_overrides(server.acp, env_provider);
    server
}

#[cfg(test)]
mod tests;
