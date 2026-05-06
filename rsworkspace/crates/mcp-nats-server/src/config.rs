use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::num::ParseIntError;

use clap::Parser;
use mcp_nats::{Config, McpPeerId, McpPeerIdError, McpPrefix, McpPrefixError, NatsConfig};
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

use crate::allowed_host::{AllowedHost, AllowedHostError};
use crate::constants::{
    DEFAULT_MCP_CLIENT_ID_PREFIX, DEFAULT_MCP_HTTP_HOST, DEFAULT_MCP_HTTP_PATH, DEFAULT_MCP_HTTP_PORT,
    DEFAULT_MCP_SERVER_ID, ENV_MCP_CLIENT_ID_PREFIX, ENV_MCP_HTTP_HOST, ENV_MCP_HTTP_PATH, ENV_MCP_HTTP_PORT,
    ENV_MCP_SERVER_ID,
};
use crate::http_route_path::{HttpRoutePath, HttpRoutePathError};

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
    #[arg(long = "path")]
    pub path: Option<String>,
    #[arg(long = "allowed-host")]
    pub allowed_hosts: Vec<String>,
}

#[derive(Clone)]
pub struct HttpBridgeConfig {
    pub mcp: Config,
    pub client_id_prefix: McpPeerId,
    pub server_id: McpPeerId,
    pub bind_addr: SocketAddr,
    pub path: HttpRoutePath,
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
    let raw_path = args
        .path
        .or_else(|| env_provider.var(ENV_MCP_HTTP_PATH).ok())
        .unwrap_or_else(|| DEFAULT_MCP_HTTP_PATH.to_string());

    let host = args
        .host
        .or(read_env_host(env_provider)?)
        .unwrap_or_else(|| DEFAULT_MCP_HTTP_HOST.parse().expect("default MCP HTTP host is valid"));
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
        path: HttpRoutePath::new(raw_path).map_err(ConfigError::Path)?,
        allowed_hosts,
    })
}

#[derive(Debug)]
pub enum ConfigError {
    Prefix(McpPrefixError),
    ClientIdPrefix(McpPeerIdError),
    ServerId(McpPeerIdError),
    Path(HttpRoutePathError),
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
            Self::Path(_) => write!(f, "invalid MCP HTTP path"),
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
            Self::Path(source) => Some(source),
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
mod tests {
    use super::*;
    use trogon_std::FixedArgs;
    use trogon_std::env::InMemoryEnv;

    fn args() -> FixedArgs<Args> {
        FixedArgs(Args {
            mcp_prefix: None,
            client_id_prefix: None,
            server_id: None,
            host: None,
            port: None,
            path: None,
            allowed_hosts: Vec::new(),
        })
    }

    fn config_from_env(env: &InMemoryEnv) -> HttpBridgeConfig {
        let config = base_config(&args(), env).unwrap();
        HttpBridgeConfig {
            mcp: mcp_nats::apply_timeout_overrides(config.mcp, env),
            client_id_prefix: config.client_id_prefix,
            server_id: config.server_id,
            bind_addr: config.bind_addr,
            path: config.path,
            allowed_hosts: config.allowed_hosts,
        }
    }

    fn err(result: Result<HttpBridgeConfig, ConfigError>) -> ConfigError {
        match result {
            Ok(_) => panic!("expected invalid MCP HTTP bridge config"),
            Err(error) => error,
        }
    }

    #[test]
    fn default_config_uses_mcp_prefix_http_peer_prefix_and_local_endpoint() {
        let config = config_from_env(&InMemoryEnv::new());

        assert_eq!(config.mcp.prefix_str(), "mcp");
        assert_eq!(config.client_id_prefix.as_str(), "http");
        assert_eq!(config.server_id.as_str(), "default");
        assert_eq!(config.bind_addr, "127.0.0.1:8081".parse().unwrap());
        assert_eq!(config.path.as_str(), "/mcp");
        assert!(config.allowed_hosts.is_empty());
    }

    #[test]
    fn reads_config_from_env_provider() {
        let env = InMemoryEnv::new();
        env.set("MCP_PREFIX", "tenant.mcp");
        env.set("MCP_CLIENT_ID_PREFIX", "browser");
        env.set("MCP_SERVER_ID", "filesystem");
        env.set("MCP_HTTP_HOST", "0.0.0.0");
        env.set("MCP_HTTP_PORT", "9090");
        env.set("MCP_HTTP_PATH", "/tenant/mcp");

        let config = config_from_env(&env);

        assert_eq!(config.mcp.prefix_str(), "tenant.mcp");
        assert_eq!(config.client_id_prefix.as_str(), "browser");
        assert_eq!(config.server_id.as_str(), "filesystem");
        assert_eq!(config.bind_addr, "0.0.0.0:9090".parse().unwrap());
        assert_eq!(config.path.as_str(), "/tenant/mcp");
    }

    #[test]
    fn args_override_env_provider() {
        let env = InMemoryEnv::new();
        env.set("MCP_PREFIX", "env-mcp");
        env.set("MCP_CLIENT_ID_PREFIX", "env-client");
        env.set("MCP_SERVER_ID", "env-server");
        env.set("MCP_HTTP_HOST", "127.0.0.1");
        env.set("MCP_HTTP_PORT", "9090");
        env.set("MCP_HTTP_PATH", "/env");
        let parser = FixedArgs(Args {
            mcp_prefix: Some("cli-mcp".to_string()),
            client_id_prefix: Some("cli-client".to_string()),
            server_id: Some("cli-server".to_string()),
            host: Some("0.0.0.0".parse().unwrap()),
            port: Some(7070),
            path: Some("/cli".to_string()),
            allowed_hosts: vec!["example.com".to_string()],
        });

        let config = base_config(&parser, &env).unwrap();

        assert_eq!(config.mcp.prefix_str(), "cli-mcp");
        assert_eq!(config.client_id_prefix.as_str(), "cli-client");
        assert_eq!(config.server_id.as_str(), "cli-server");
        assert_eq!(config.bind_addr, "0.0.0.0:7070".parse().unwrap());
        assert_eq!(config.path.as_str(), "/cli");
        assert_eq!(
            config.allowed_hosts.iter().map(AllowedHost::as_str).collect::<Vec<_>>(),
            vec!["example.com"]
        );
    }

    #[test]
    fn reads_nats_config_from_env_provider() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222,host2:4222");
        env.set("NATS_TOKEN", "my-token");

        let config = config_from_env(&env);

        assert_eq!(config.mcp.nats().servers, vec!["host1:4222", "host2:4222"]);
        assert!(matches!(&config.mcp.nats().auth, mcp_nats::NatsAuth::Token(token) if token == "my-token"));
    }

    #[test]
    fn rejects_invalid_values() {
        let env = InMemoryEnv::new();

        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: Some("bad.*".to_string()),
                    client_id_prefix: None,
                    server_id: None,
                    host: None,
                    port: None,
                    path: None,
                    allowed_hosts: Vec::new(),
                }),
                &env,
            ),
            Err(ConfigError::Prefix(_))
        ));
        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: None,
                    client_id_prefix: Some("bad.client".to_string()),
                    server_id: None,
                    host: None,
                    port: None,
                    path: None,
                    allowed_hosts: Vec::new(),
                }),
                &env,
            ),
            Err(ConfigError::ClientIdPrefix(_))
        ));
        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: None,
                    client_id_prefix: None,
                    server_id: Some("bad.server".to_string()),
                    host: None,
                    port: None,
                    path: None,
                    allowed_hosts: Vec::new(),
                }),
                &env,
            ),
            Err(ConfigError::ServerId(_))
        ));
        env.set("MCP_HTTP_HOST", "bad-host");
        assert!(matches!(
            base_config(&args(), &env),
            Err(ConfigError::InvalidHost { .. })
        ));
        env.remove("MCP_HTTP_HOST");
        env.set("MCP_HTTP_PORT", "bad-port");
        assert!(matches!(
            base_config(&args(), &env),
            Err(ConfigError::InvalidPort { .. })
        ));
        env.remove("MCP_HTTP_PORT");
        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: None,
                    client_id_prefix: None,
                    server_id: None,
                    host: None,
                    port: None,
                    path: Some("mcp".to_string()),
                    allowed_hosts: Vec::new(),
                }),
                &env,
            ),
            Err(ConfigError::Path(_))
        ));
        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: None,
                    client_id_prefix: None,
                    server_id: None,
                    host: None,
                    port: None,
                    path: None,
                    allowed_hosts: vec!["bad host".to_string()],
                }),
                &env,
            ),
            Err(ConfigError::AllowedHost(_))
        ));
    }

    #[test]
    fn config_error_display_and_source_are_specific() {
        assert_eq!(
            HttpRoutePathError.to_string(),
            "HTTP route path must start with '/', include a route segment, and contain only valid path characters"
        );

        let prefix = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: Some("bad.*".to_string()),
                client_id_prefix: None,
                server_id: None,
                host: None,
                port: None,
                path: None,
                allowed_hosts: Vec::new(),
            }),
            &InMemoryEnv::new(),
        ));
        assert_eq!(prefix.to_string(), "invalid MCP prefix");
        assert!(std::error::Error::source(&prefix).is_some());

        let client_id_prefix = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id_prefix: Some("bad.client".to_string()),
                server_id: None,
                host: None,
                port: None,
                path: None,
                allowed_hosts: Vec::new(),
            }),
            &InMemoryEnv::new(),
        ));
        assert_eq!(client_id_prefix.to_string(), "invalid MCP client id prefix");
        assert!(std::error::Error::source(&client_id_prefix).is_some());

        let server_id = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id_prefix: None,
                server_id: Some("bad.server".to_string()),
                host: None,
                port: None,
                path: None,
                allowed_hosts: Vec::new(),
            }),
            &InMemoryEnv::new(),
        ));
        assert_eq!(server_id.to_string(), "invalid MCP server id");
        assert!(std::error::Error::source(&server_id).is_some());

        let path = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id_prefix: None,
                server_id: None,
                host: None,
                port: None,
                path: Some("/".to_string()),
                allowed_hosts: Vec::new(),
            }),
            &InMemoryEnv::new(),
        ));

        assert_eq!(path.to_string(), "invalid MCP HTTP path");
        assert!(std::error::Error::source(&path).is_some());

        let allowed_host = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id_prefix: None,
                server_id: None,
                host: None,
                port: None,
                path: None,
                allowed_hosts: vec!["bad host".to_string()],
            }),
            &InMemoryEnv::new(),
        ));
        assert_eq!(allowed_host.to_string(), "invalid MCP HTTP allowed host");
        assert!(std::error::Error::source(&allowed_host).is_some());

        let env = InMemoryEnv::new();
        env.set("MCP_HTTP_HOST", "bad-host");
        let invalid_host = err(base_config(&args(), &env));
        assert_eq!(
            invalid_host.to_string(),
            "invalid value for MCP_HTTP_HOST: \"bad-host\" (invalid IP address syntax)"
        );
        assert!(std::error::Error::source(&invalid_host).is_some());

        let env = InMemoryEnv::new();
        env.set("MCP_HTTP_PORT", "bad-port");
        let invalid_port = err(base_config(&args(), &env));

        assert_eq!(
            invalid_port.to_string(),
            "invalid value for MCP_HTTP_PORT: \"bad-port\" (invalid digit found in string)"
        );
        assert!(std::error::Error::source(&invalid_port).is_some());
    }

    #[test]
    #[should_panic(expected = "expected invalid MCP HTTP bridge config")]
    fn err_panics_when_config_is_valid() {
        let _ = err(base_config(&args(), &InMemoryEnv::new()));
    }
}
