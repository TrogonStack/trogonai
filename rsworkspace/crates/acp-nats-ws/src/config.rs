use acp_nats::{AcpPrefix, AcpPrefixError, Config, NatsConfig};
use clap::Parser;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use tracing::warn;
use trogon_std::env::ReadEnv;

const MIN_TIMEOUT_SECS: u64 = 1;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

const ENV_ACP_PREFIX: &str = "ACP_PREFIX";
const ENV_OPERATION_TIMEOUT_SECS: &str = "ACP_OPERATION_TIMEOUT_SECS";
const ENV_PROMPT_TIMEOUT_SECS: &str = "ACP_PROMPT_TIMEOUT_SECS";
const ENV_CONNECT_TIMEOUT_SECS: &str = "ACP_NATS_CONNECT_TIMEOUT_SECS";
const DEFAULT_ACP_PREFIX: &str = "acp";

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

pub fn base_config<E: ReadEnv>(env_provider: &E) -> Result<WsConfig, AcpPrefixError> {
    let args = Args::parse();
    base_config_from_args(args, env_provider)
}

fn base_config_from_args<E: ReadEnv>(
    args: Args,
    env_provider: &E,
) -> Result<WsConfig, AcpPrefixError> {
    let raw_prefix = args
        .acp_prefix
        .or_else(|| env_provider.var(ENV_ACP_PREFIX).ok())
        .unwrap_or_else(|| DEFAULT_ACP_PREFIX.to_string());
    let prefix = AcpPrefix::new(raw_prefix)?;
    Ok(WsConfig {
        acp: Config::with_prefix(prefix, NatsConfig::from_env(env_provider)),
        host: args.host,
        port: args.port,
    })
}

pub fn apply_timeout_overrides<E: ReadEnv>(mut ws: WsConfig, env_provider: &E) -> WsConfig {
    let mut config = ws.acp;

    if let Ok(raw) = env_provider.var(ENV_OPERATION_TIMEOUT_SECS) {
        match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => {
                config = config.with_operation_timeout(Duration::from_secs(secs));
            }
            Ok(secs) => {
                warn!(
                    "{ENV_OPERATION_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
            }
            Err(_) => {
                warn!("{ENV_OPERATION_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
            }
        }
    }

    if let Ok(raw) = env_provider.var(ENV_PROMPT_TIMEOUT_SECS) {
        match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => {
                config = config.with_prompt_timeout(Duration::from_secs(secs));
            }
            Ok(secs) => {
                warn!(
                    "{ENV_PROMPT_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
            }
            Err(_) => {
                warn!("{ENV_PROMPT_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
            }
        }
    }

    ws.acp = config;
    ws
}

pub fn nats_connect_timeout<E: ReadEnv>(env_provider: &E) -> Duration {
    let default = Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS);

    match env_provider.var(ENV_CONNECT_TIMEOUT_SECS) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(secs) if secs >= MIN_TIMEOUT_SECS => Duration::from_secs(secs),
            Ok(secs) => {
                warn!(
                    "{ENV_CONNECT_TIMEOUT_SECS}={secs} is below minimum ({MIN_TIMEOUT_SECS}), using default"
                );
                default
            }
            Err(_) => {
                warn!("{ENV_CONNECT_TIMEOUT_SECS}={raw:?} is not a valid integer, using default");
                default
            }
        },
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_std::env::InMemoryEnv;

    fn config_from_env(env: &InMemoryEnv) -> WsConfig {
        let args = Args {
            acp_prefix: None,
            host: DEFAULT_HOST,
            port: DEFAULT_PORT,
        };
        let ws = base_config_from_args(args, env).unwrap();
        apply_timeout_overrides(ws, env)
    }

    #[test]
    fn test_default_config() {
        let env = InMemoryEnv::new();
        let ws = config_from_env(&env);
        assert_eq!(ws.acp.acp_prefix(), DEFAULT_ACP_PREFIX);
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
        let ws = base_config_from_args(args, &env).unwrap();
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
        let ws = base_config_from_args(args, &env).unwrap();
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
    fn test_nats_connect_timeout_from_env() {
        with_subscriber(|| {
            let env = InMemoryEnv::new();
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );

            env.set(ENV_CONNECT_TIMEOUT_SECS, "15");
            assert_eq!(nats_connect_timeout(&env), Duration::from_secs(15));
            env.set(ENV_CONNECT_TIMEOUT_SECS, "0");
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );
            env.set(ENV_CONNECT_TIMEOUT_SECS, "not-a-number");
            assert_eq!(
                nats_connect_timeout(&env),
                Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
            );
        });
    }

    fn with_subscriber<F: FnOnce()>(f: F) {
        let _guard = tracing_subscriber::fmt().with_test_writer().set_default();
        f();
    }

    #[test]
    fn test_operation_timeout_invalid_env_is_ignored() {
        with_subscriber(|| {
            let env = InMemoryEnv::new();
            let default = config_from_env(&env);
            let default_timeout = default.acp.operation_timeout();

            env.set("ACP_OPERATION_TIMEOUT_SECS", "not-a-number");
            let invalid = config_from_env(&env);
            assert_eq!(invalid.acp.operation_timeout(), default_timeout);
        });
    }

    #[test]
    fn test_operation_timeout_under_min_is_ignored() {
        with_subscriber(|| {
            let env = InMemoryEnv::new();
            let default = config_from_env(&env);
            let default_timeout = default.acp.operation_timeout();

            env.set("ACP_OPERATION_TIMEOUT_SECS", "0");
            let ignored = config_from_env(&env);
            assert_eq!(ignored.acp.operation_timeout(), default_timeout);
        });
    }

    #[test]
    fn test_prompt_timeout_invalid_env_is_ignored() {
        with_subscriber(|| {
            let env = InMemoryEnv::new();
            let default = config_from_env(&env);
            let default_timeout = default.acp.prompt_timeout();

            env.set("ACP_PROMPT_TIMEOUT_SECS", "not-a-number");
            let invalid = config_from_env(&env);
            assert_eq!(invalid.acp.prompt_timeout(), default_timeout);
        });
    }

    #[test]
    fn test_prompt_timeout_under_min_is_ignored() {
        with_subscriber(|| {
            let env = InMemoryEnv::new();
            let default = config_from_env(&env);
            let default_timeout = default.acp.prompt_timeout();

            env.set("ACP_PROMPT_TIMEOUT_SECS", "0");
            let ignored = config_from_env(&env);
            assert_eq!(ignored.acp.prompt_timeout(), default_timeout);
        });
    }

    #[test]
    fn test_custom_host_and_port() {
        let env = InMemoryEnv::new();
        let args = Args {
            acp_prefix: None,
            host: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            port: 9090,
        };
        let ws = base_config_from_args(args, &env).unwrap();
        assert_eq!(ws.host, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(ws.port, 9090);
    }
}
