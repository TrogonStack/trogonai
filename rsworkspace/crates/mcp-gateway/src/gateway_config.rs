use clap::Parser;
use mcp_nats::{McpPeerId, McpPeerIdError, McpPrefix, McpPrefixError};
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::ResponsePolicy;

const ENV_RESPONSE_POLICY: &str = "MCP_GATEWAY_RESPONSE_POLICY";

/// CLI/env surface for the `mcp-gateway` binary, mirroring a2a-gateway's
/// `Args` shape: a small clap surface, with the MCP-specific knobs (prefix,
/// public/upstream server identity, response policy) each resolved through
/// their own validated value object.
#[derive(Parser, Debug)]
#[command(name = "mcp-gateway")]
#[command(about = "MCP security gateway that intercepts tools/list and tools/call traffic", long_about = None)]
pub struct Args {
    /// Comma-separated NATS server URL(s). Overrides `NATS_URL` when passed explicitly.
    #[arg(long, env = "NATS_URL", default_value = "localhost:4222")]
    pub nats_url: String,

    /// MCP subject prefix shared with the real server and clients.
    #[arg(long, env = "MCP_PREFIX", default_value = "mcp")]
    pub prefix: String,

    /// Server identity the gateway exposes to MCP clients (the public,
    /// gateway-fronted identity).
    #[arg(long, env = "MCP_GATEWAY_PUBLIC_SERVER_ID")]
    pub public_server_id: String,

    /// Server identity of the real upstream MCP server the gateway forwards
    /// clean traffic to.
    #[arg(long, env = "MCP_GATEWAY_UPSTREAM_SERVER_ID")]
    pub upstream_server_id: String,

    /// How the gateway handles threats found in a tool response: `block`,
    /// `sanitize`, or `log`. Defaults to `block` (fail closed).
    #[arg(long, env = "MCP_GATEWAY_RESPONSE_POLICY")]
    pub response_policy: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub nats_servers: Vec<String>,
    pub mcp_prefix: McpPrefix,
    pub public_server_id: McpPeerId,
    pub upstream_server_id: McpPeerId,
    pub response_policy: ResponsePolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid MCP prefix")]
    InvalidPrefix(#[source] McpPrefixError),
    #[error("invalid public server id")]
    InvalidPublicServerId(#[source] McpPeerIdError),
    #[error("invalid upstream server id")]
    InvalidUpstreamServerId(#[source] McpPeerIdError),
    #[error("--nats-url / NATS_URL must list at least one server, got {raw:?}")]
    EmptyNatsServers { raw: String },
    #[error(
        "invalid response policy {raw:?} (expected \"block\", \"sanitize\", or \"log\") via --response-policy / {ENV_RESPONSE_POLICY}"
    )]
    InvalidResponsePolicy { raw: String },
}

pub fn config_from_args<E: ReadEnv>(args: Args, env: &E) -> Result<(Config, NatsConfig), ConfigError> {
    let mcp_prefix = McpPrefix::new(args.prefix).map_err(ConfigError::InvalidPrefix)?;
    let public_server_id = McpPeerId::new(args.public_server_id).map_err(ConfigError::InvalidPublicServerId)?;
    let upstream_server_id = McpPeerId::new(args.upstream_server_id).map_err(ConfigError::InvalidUpstreamServerId)?;

    let nats_servers = parse_servers(&args.nats_url)?;
    let mut nats_config = NatsConfig::from_env(env);
    nats_config.servers.clone_from(&nats_servers);

    let response_policy_raw = args.response_policy.or_else(|| env.var(ENV_RESPONSE_POLICY).ok());
    let response_policy = match response_policy_raw {
        Some(raw) => parse_response_policy(&raw)?,
        None => ResponsePolicy::default(),
    };

    let config = Config {
        nats_servers,
        mcp_prefix,
        public_server_id,
        upstream_server_id,
        response_policy,
    };

    Ok((config, nats_config))
}

fn parse_servers(raw: &str) -> Result<Vec<String>, ConfigError> {
    let servers: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .map(str::to_owned)
        .collect();
    if servers.is_empty() {
        return Err(ConfigError::EmptyNatsServers { raw: raw.to_owned() });
    }
    Ok(servers)
}

fn parse_response_policy(raw: &str) -> Result<ResponsePolicy, ConfigError> {
    match raw.trim().to_lowercase().as_str() {
        "block" => Ok(ResponsePolicy::Block),
        "sanitize" => Ok(ResponsePolicy::Sanitize),
        "log" => Ok(ResponsePolicy::Log),
        _ => Err(ConfigError::InvalidResponsePolicy { raw: raw.to_owned() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    fn args() -> Args {
        Args {
            nats_url: "localhost:4222".to_string(),
            prefix: "mcp".to_string(),
            public_server_id: "gateway".to_string(),
            upstream_server_id: "filesystem".to_string(),
            response_policy: None,
        }
    }

    #[test]
    fn resolves_defaults_with_no_env_overrides() {
        let env = InMemoryEnv::new();
        let (config, _) = config_from_args(args(), &env).expect("valid config");
        assert_eq!(config.mcp_prefix.as_str(), "mcp");
        assert_eq!(config.public_server_id.as_str(), "gateway");
        assert_eq!(config.upstream_server_id.as_str(), "filesystem");
        assert_eq!(config.response_policy, ResponsePolicy::Block);
    }

    #[test]
    fn parses_comma_separated_nats_urls() {
        let env = InMemoryEnv::new();
        let mut a = args();
        a.nats_url = "nats1:4222, nats2:4222".to_string();
        let (config, _) = config_from_args(a, &env).expect("valid config");
        assert_eq!(config.nats_servers, vec!["nats1:4222", "nats2:4222"]);
    }

    #[test]
    fn rejects_empty_nats_url() {
        let env = InMemoryEnv::new();
        let mut a = args();
        a.nats_url = "   ".to_string();
        let err = config_from_args(a, &env).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyNatsServers { .. }));
    }

    #[test]
    fn rejects_invalid_public_server_id() {
        let env = InMemoryEnv::new();
        let mut a = args();
        a.public_server_id = String::new();
        let err = config_from_args(a, &env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidPublicServerId(_)));
    }

    #[test]
    fn parses_response_policy_from_cli() {
        let env = InMemoryEnv::new();
        let mut a = args();
        a.response_policy = Some("log".to_string());
        let (config, _) = config_from_args(a, &env).expect("valid config");
        assert_eq!(config.response_policy, ResponsePolicy::Log);
    }

    #[test]
    fn rejects_unknown_response_policy() {
        let env = InMemoryEnv::new();
        let mut a = args();
        a.response_policy = Some("nonsense".to_string());
        let err = config_from_args(a, &env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidResponsePolicy { .. }));
    }
}
