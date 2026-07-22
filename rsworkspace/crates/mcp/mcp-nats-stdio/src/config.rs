use clap::Parser;
use mcp_nats::{Config, McpPeerId, McpPeerIdError, McpPrefix, McpPrefixError, NatsConfig};
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

pub const DEFAULT_MCP_CLIENT_ID: &str = "stdio";
pub const DEFAULT_MCP_SERVER_ID: &str = "default";
pub const ENV_MCP_CLIENT_ID: &str = "MCP_CLIENT_ID";
pub const ENV_MCP_SERVER_ID: &str = "MCP_SERVER_ID";

#[derive(Parser, Debug, Clone)]
#[command(name = "mcp-nats-stdio")]
#[command(about = "MCP stdio to NATS bridge", long_about = None)]
pub struct Args {
    #[arg(long = "mcp-prefix")]
    pub mcp_prefix: Option<String>,
    #[arg(long = "client-id")]
    pub client_id: Option<String>,
    #[arg(long = "server-id")]
    pub server_id: Option<String>,
}

#[derive(Clone)]
pub struct BridgeConfig {
    pub mcp: Config,
    pub client_id: McpPeerId,
    pub server_id: McpPeerId,
}

pub fn base_config<P: ParseArgs<Args = Args>, E: ReadEnv>(
    parser: &P,
    env_provider: &E,
) -> Result<BridgeConfig, ConfigError> {
    let args = parser.parse_args();
    base_config_from_args(args, env_provider)
}

fn base_config_from_args<E: ReadEnv>(args: Args, env_provider: &E) -> Result<BridgeConfig, ConfigError> {
    let raw_prefix = args
        .mcp_prefix
        .or_else(|| env_provider.var(mcp_nats::ENV_MCP_PREFIX).ok())
        .unwrap_or_else(|| mcp_nats::DEFAULT_MCP_PREFIX.to_string());
    let raw_client_id = args
        .client_id
        .or_else(|| env_provider.var(ENV_MCP_CLIENT_ID).ok())
        .unwrap_or_else(|| DEFAULT_MCP_CLIENT_ID.to_string());
    let raw_server_id = args
        .server_id
        .or_else(|| env_provider.var(ENV_MCP_SERVER_ID).ok())
        .unwrap_or_else(|| DEFAULT_MCP_SERVER_ID.to_string());

    Ok(BridgeConfig {
        mcp: Config::new(
            McpPrefix::new(raw_prefix).map_err(ConfigError::Prefix)?,
            NatsConfig::from_env(env_provider),
        ),
        client_id: McpPeerId::new(raw_client_id).map_err(ConfigError::ClientId)?,
        server_id: McpPeerId::new(raw_server_id).map_err(ConfigError::ServerId)?,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid MCP prefix")]
    Prefix(#[source] McpPrefixError),
    #[error("invalid MCP client id")]
    ClientId(#[source] McpPeerIdError),
    #[error("invalid MCP server id")]
    ServerId(#[source] McpPeerIdError),
}

#[cfg(test)]
mod tests;
