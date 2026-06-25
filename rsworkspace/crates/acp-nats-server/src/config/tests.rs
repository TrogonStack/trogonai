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
fn test_invalid_acp_prefix_maps_to_server_config_error() {
    let env = InMemoryEnv::new();
    let args = Args {
        acp_prefix: Some("acp.*".into()),
        host: None,
        port: None,
    };
    let err = config_from_args(args, &env)
        .err()
        .expect("invalid ACP prefix should fail");
    assert!(matches!(err, ServerConfigError::InvalidAcpPrefix(_)));
    assert!(!err.to_string().is_empty());
}

#[test]
fn test_invalid_new_server_env_var_fails() {
    let env = InMemoryEnv::new();
    env.set("ACP_SERVER_PORT", "abc");
    let error = config_from_args(
        Args {
            acp_prefix: None,
            host: None,
            port: None,
        },
        &env,
    )
    .err()
    .expect("invalid ACP_SERVER_PORT should fail");
    assert_eq!(
        format!("{error}"),
        "invalid value for ACP_SERVER_PORT: \"abc\" (invalid digit found in string)"
    );
}
