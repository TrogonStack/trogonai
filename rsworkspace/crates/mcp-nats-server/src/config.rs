use std::net::{AddrParseError, IpAddr, Ipv4Addr, SocketAddr};
use std::num::ParseIntError;

use clap::Parser;
use mcp_nats::{Config, McpPeerId, McpPeerIdError, McpPrefix, McpPrefixError, NatsConfig};
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

use crate::allowed_host::{AllowedHost, AllowedHostError};
use crate::constants::{
    DEFAULT_MCP_CLIENT_ID_PREFIX, DEFAULT_MCP_HTTP_PORT, DEFAULT_MCP_SERVER_ID, ENV_MCP_CLIENT_ID_PREFIX,
    ENV_MCP_HTTP_HOST, ENV_MCP_HTTP_PORT, ENV_MCP_SERVER_ID,
};

#[derive(Parser, Debug, Clone)]
#[command(name = "mcp-nats-server")]
#[command(about = "MCP Streamable HTTP to NATS bridge", long_about = None)]
pub struct Args {
    #[arg(long = "mcp-prefix")]
    pub mcp_prefix: Option<String>,
    #[arg(long = "client-id-prefix")]
    pub client_id_prefix: Option<String>,
    #[arg(long = "server-id")]
    pub server_id: Option<String>,
    #[arg(long = "host")]
    pub host: Option<IpAddr>,
    #[arg(long = "port")]
    pub port: Option<u16>,
    #[arg(long = "allowed-host")]
    pub allowed_hosts: Vec<String>,
}

#[derive(Clone)]
pub struct HttpBridgeConfig {
    pub mcp: Config,
    pub client_id_prefix: McpPeerId,
    pub server_id: McpPeerId,
    pub bind_addr: SocketAddr,
    pub allowed_hosts: Vec<AllowedHost>,
}

pub fn base_config<P: ParseArgs<Args = Args>, E: ReadEnv>(
    parser: &P,
    env_provider: &E,
) -> Result<HttpBridgeConfig, ConfigError> {
    let args = parser.parse_args();
    base_config_from_args(args, env_provider)
}

fn base_config_from_args<E: ReadEnv>(args: Args, env_provider: &E) -> Result<HttpBridgeConfig, ConfigError> {
    let raw_prefix = args
        .mcp_prefix
        .or_else(|| env_provider.var(mcp_nats::ENV_MCP_PREFIX).ok())
        .unwrap_or_else(|| mcp_nats::DEFAULT_MCP_PREFIX.to_string());
    let raw_client_id_prefix = args
        .client_id_prefix
        .or_else(|| env_provider.var(ENV_MCP_CLIENT_ID_PREFIX).ok())
        .unwrap_or_else(|| DEFAULT_MCP_CLIENT_ID_PREFIX.to_string());
    let raw_server_id = args
        .server_id
        .or_else(|| env_provider.var(ENV_MCP_SERVER_ID).ok())
        .unwrap_or_else(|| DEFAULT_MCP_SERVER_ID.to_string());
    let port = args
        .port
        .or(read_env_port(env_provider)?)
        .unwrap_or(DEFAULT_MCP_HTTP_PORT);

    let host = args
        .host
        .or(read_env_host(env_provider)?)
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let allowed_hosts = args
        .allowed_hosts
        .into_iter()
        .map(AllowedHost::new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(ConfigError::AllowedHost)?;

    Ok(HttpBridgeConfig {
        mcp: Config::new(
            McpPrefix::new(raw_prefix).map_err(ConfigError::Prefix)?,
            NatsConfig::from_env(env_provider),
        ),
        client_id_prefix: McpPeerId::new(raw_client_id_prefix).map_err(ConfigError::ClientIdPrefix)?,
        server_id: McpPeerId::new(raw_server_id).map_err(ConfigError::ServerId)?,
        bind_addr: SocketAddr::new(host, port),
        allowed_hosts,
    })
}

#[derive(Debug)]
pub enum ConfigError {
    Prefix(McpPrefixError),
    ClientIdPrefix(McpPeerIdError),
    ServerId(McpPeerIdError),
    AllowedHost(AllowedHostError),
    InvalidHost { value: String, source: AddrParseError },
    InvalidPort { value: String, source: ParseIntError },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Prefix(_) => write!(f, "invalid MCP prefix"),
            Self::ClientIdPrefix(_) => write!(f, "invalid MCP client id prefix"),
            Self::ServerId(_) => write!(f, "invalid MCP server id"),
            Self::AllowedHost(_) => write!(f, "invalid MCP HTTP allowed host"),
            Self::InvalidHost { value, source } => {
                write!(f, "invalid value for {ENV_MCP_HTTP_HOST}: {value:?} ({source})")
            }
            Self::InvalidPort { value, source } => {
                write!(f, "invalid value for {ENV_MCP_HTTP_PORT}: {value:?} ({source})")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Prefix(source) => Some(source),
            Self::ClientIdPrefix(source) | Self::ServerId(source) => Some(source),
            Self::AllowedHost(source) => Some(source),
            Self::InvalidHost { source, .. } => Some(source),
            Self::InvalidPort { source, .. } => Some(source),
        }
    }
}

fn read_env_host<E: ReadEnv>(env_provider: &E) -> Result<Option<IpAddr>, ConfigError> {
    match env_provider.var(ENV_MCP_HTTP_HOST) {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|source| ConfigError::InvalidHost { value, source }),
        Err(_) => Ok(None),
    }
}

fn read_env_port<E: ReadEnv>(env_provider: &E) -> Result<Option<u16>, ConfigError> {
    match env_provider.var(ENV_MCP_HTTP_PORT) {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|source| ConfigError::InvalidPort { value, source }),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests;
