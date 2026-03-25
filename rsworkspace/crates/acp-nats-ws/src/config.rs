use acp_nats::{AcpPrefix, AcpPrefixError, Config, NatsConfig};
use clap::Parser;
use std::net::{IpAddr, Ipv4Addr};
use trogon_std::env::ReadEnv;

const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
const DEFAULT_PORT: u16 = 8080;

#[derive(Parser, Debug)]
#[command(name = "acp-nats-ws")]
#[command(about = "ACP WebSocket to NATS bridge for agent-client protocol", long_about = None)]
pub struct Args {
    #[arg(long = "acp-prefix")]
    pub acp_prefix: Option<String>,

    #[arg(long, env = "ACP_WS_HOST", default_value_t = DEFAULT_HOST)]
    pub host: IpAddr,

    #[arg(long, env = "ACP_WS_PORT", default_value_t = DEFAULT_PORT)]
    pub port: u16,
}

pub struct WsConfig {
    pub acp: Config,
    pub host: IpAddr,
    pub port: u16,
}

#[cfg_attr(coverage, coverage(off))]
pub fn config_from_args<E: ReadEnv>(
    args: Args,
    env_provider: &E,
) -> Result<WsConfig, AcpPrefixError> {
    let raw_prefix = args
        .acp_prefix
        .or_else(|| env_provider.var(acp_nats::ENV_ACP_PREFIX).ok())
        .unwrap_or_else(|| acp_nats::DEFAULT_ACP_PREFIX.to_string());
    let prefix = AcpPrefix::new(raw_prefix)?;
    Ok(WsConfig {
        acp: Config::with_prefix(prefix, NatsConfig::from_env(env_provider)),
        host: args.host,
        port: args.port,
    })
}

#[cfg_attr(coverage, coverage(off))]
pub fn apply_timeout_overrides<E: ReadEnv>(mut ws: WsConfig, env_provider: &E) -> WsConfig {
    ws.acp = acp_nats::apply_timeout_overrides(ws.acp, env_provider);
    ws
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    fn config_from_env(env: &InMemoryEnv) -> WsConfig {
        let args = Args {
            acp_prefix: None,
            host: DEFAULT_HOST,
            port: DEFAULT_PORT,
        };
        let ws = config_from_args(args, env).unwrap();
        apply_timeout_overrides(ws, env)
    }

    #[test]
    fn test_default_config() {
        let env = InMemoryEnv::new();
        let ws = config_from_env(&env);
        assert_eq!(ws.acp.acp_prefix(), acp_nats::DEFAULT_ACP_PREFIX);
        assert_eq!(ws.host, DEFAULT_HOST);
        assert_eq!(ws.port, DEFAULT_PORT);
        assert_eq!(ws.acp.nats().servers, vec!["localhost:4222"]);
        assert!(matches!(&ws.acp.nats().auth, acp_nats::NatsAuth::None));
    }

    #[test]
    fn test_acp_prefix_from_env_provider() {
        let env = InMemoryEnv::new();
        env.set("ACP_PREFIX", "custom-prefix");
        let ws = config_from_env(&env);
        assert_eq!(ws.acp.acp_prefix(), "custom-prefix");
    }

    #[test]
    fn test_acp_prefix_from_args() {
        let env = InMemoryEnv::new();
        let args = Args {
            acp_prefix: Some("cli-prefix".to_string()),
            host: DEFAULT_HOST,
            port: DEFAULT_PORT,
        };
        let ws = config_from_args(args, &env).unwrap();
        assert_eq!(ws.acp.acp_prefix(), "cli-prefix");
    }

    #[test]
    fn test_args_override_env() {
        let env = InMemoryEnv::new();
        env.set("ACP_PREFIX", "env-prefix");
        let args = Args {
            acp_prefix: Some("cli-prefix".to_string()),
            host: DEFAULT_HOST,
            port: DEFAULT_PORT,
        };
        let ws = config_from_args(args, &env).unwrap();
        assert_eq!(ws.acp.acp_prefix(), "cli-prefix");
    }

    #[test]
    fn test_nats_config_from_env() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222,host2:4222");
        env.set("NATS_TOKEN", "my-token");
        let ws = config_from_env(&env);
        assert_eq!(ws.acp.nats().servers, vec!["host1:4222", "host2:4222"]);
        assert!(matches!(&ws.acp.nats().auth, acp_nats::NatsAuth::Token(t) if t == "my-token"));
    }

    #[test]
    fn test_custom_host_and_port() {
        let env = InMemoryEnv::new();
        let args = Args {
            acp_prefix: None,
            host: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            port: 9090,
        };
        let ws = config_from_args(args, &env).unwrap();
        assert_eq!(ws.host, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(ws.port, 9090);
    }
}
