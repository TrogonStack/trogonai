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

pub fn config_from_args<E: ReadEnv>(args: Args, env_provider: &E) -> Result<ServerConfig, AcpPrefixError> {
    let raw_prefix = args
        .acp_prefix
        .or_else(|| env_provider.var(acp_nats::ENV_ACP_PREFIX).ok())
        .unwrap_or_else(|| acp_nats::DEFAULT_ACP_PREFIX.to_string());
    let prefix = AcpPrefix::new(raw_prefix)?;
    Ok(ServerConfig {
        acp: Config::with_prefix(prefix, NatsConfig::from_env(env_provider)),
        host: args
            .host
            .or_else(|| env_var(env_provider, "ACP_SERVER_HOST"))
            .or_else(|| env_var(env_provider, "ACP_WS_HOST"))
            .unwrap_or(DEFAULT_HOST),
        port: args
            .port
            .or_else(|| env_var(env_provider, "ACP_SERVER_PORT"))
            .or_else(|| env_var(env_provider, "ACP_WS_PORT"))
            .unwrap_or(DEFAULT_PORT),
    })
}

fn env_var<T: std::str::FromStr, E: ReadEnv>(env_provider: &E, key: &str) -> Option<T> {
    env_provider.var(key).ok()?.parse().ok()
}

pub fn apply_timeout_overrides<E: ReadEnv>(mut server: ServerConfig, env_provider: &E) -> ServerConfig {
    server.acp = acp_nats::apply_timeout_overrides(server.acp, env_provider);
    server
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use trogon_std::env::InMemoryEnv;

    fn config_from_env(env: &InMemoryEnv) -> ServerConfig {
        let args = Args {
            acp_prefix: None,
            host: None,
            port: None,
        };
        let server = config_from_args(args, env).unwrap();
        apply_timeout_overrides(server, env)
    }

    #[test]
    fn test_default_config() {
        let env = InMemoryEnv::new();
        let server = config_from_env(&env);
        assert_eq!(server.acp.acp_prefix(), acp_nats::DEFAULT_ACP_PREFIX);
        assert_eq!(server.host, DEFAULT_HOST);
        assert_eq!(server.port, DEFAULT_PORT);
        assert_eq!(server.acp.nats().servers, vec!["localhost:4222"]);
        assert!(matches!(&server.acp.nats().auth, acp_nats::NatsAuth::None));
    }

    #[test]
    fn test_acp_prefix_from_env_provider() {
        let env = InMemoryEnv::new();
        env.set("ACP_PREFIX", "custom-prefix");
        let server = config_from_env(&env);
        assert_eq!(server.acp.acp_prefix(), "custom-prefix");
    }

    #[test]
    fn test_acp_prefix_from_args() {
        let env = InMemoryEnv::new();
        let args = Args {
            acp_prefix: Some("cli-prefix".to_string()),
            host: None,
            port: None,
        };
        let server = config_from_args(args, &env).unwrap();
        assert_eq!(server.acp.acp_prefix(), "cli-prefix");
    }

    #[test]
    fn test_args_override_env() {
        let env = InMemoryEnv::new();
        env.set("ACP_PREFIX", "env-prefix");
        let args = Args {
            acp_prefix: Some("cli-prefix".to_string()),
            host: None,
            port: None,
        };
        let server = config_from_args(args, &env).unwrap();
        assert_eq!(server.acp.acp_prefix(), "cli-prefix");
    }

    #[test]
    fn test_nats_config_from_env() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222,host2:4222");
        env.set("NATS_TOKEN", "my-token");
        let server = config_from_env(&env);
        assert_eq!(server.acp.nats().servers, vec!["host1:4222", "host2:4222"]);
        assert!(matches!(&server.acp.nats().auth, acp_nats::NatsAuth::Token(t) if t == "my-token"));
    }

    #[test]
    fn test_custom_host_and_port() {
        let env = InMemoryEnv::new();
        let args = Args {
            acp_prefix: None,
            host: Some(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))),
            port: Some(9090),
        };
        let server = config_from_args(args, &env).unwrap();
        assert_eq!(server.host, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(server.port, 9090);
    }

    #[test]
    fn test_new_server_env_vars_override_defaults() {
        let env = InMemoryEnv::new();
        env.set("ACP_SERVER_HOST", "0.0.0.0");
        env.set("ACP_SERVER_PORT", "9091");
        let server = config_from_env(&env);
        assert_eq!(server.host, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(server.port, 9091);
    }

    #[test]
    fn test_legacy_ws_env_vars_still_work() {
        let env = InMemoryEnv::new();
        env.set("ACP_WS_HOST", "0.0.0.0");
        env.set("ACP_WS_PORT", "9092");
        let server = config_from_env(&env);
        assert_eq!(server.host, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(server.port, 9092);
    }
}
