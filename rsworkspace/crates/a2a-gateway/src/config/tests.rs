use trogon_std::env::InMemoryEnv;

use super::*;

#[test]
fn config_from_args_rejects_invalid_prefix() {
    let env = InMemoryEnv::new();
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "bad prefix!".to_string(),
        queue_group: None,
    };
    let result = config_from_args(args, &env);
    assert!(matches!(result, Err(ConfigError::InvalidPrefix(_))));
}

#[test]
fn config_gateway_subscribe_subject_uses_prefix() {
    let env = InMemoryEnv::new();
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "a2a".to_string(),
        queue_group: Some("gateways".to_string()),
    };
    let (config, _) = config_from_args(args, &env).expect("valid args");
    assert_eq!(config.gateway_subscribe_subject(), "a2a.gateway.>");
    assert_eq!(config.queue_group.as_deref(), Some("gateways"));
}

#[test]
fn config_from_args_falls_back_to_env_queue_group() {
    let env = InMemoryEnv::new();
    env.set("A2A_GATEWAY_QUEUE_GROUP", "from-env");
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "a2a".to_string(),
        queue_group: None,
    };
    let (config, _) = config_from_args(args, &env).expect("valid args");
    assert_eq!(config.queue_group.as_deref(), Some("from-env"));
}

#[test]
fn config_from_args_args_override_env_queue_group() {
    let env = InMemoryEnv::new();
    env.set("A2A_GATEWAY_QUEUE_GROUP", "from-env");
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "a2a".to_string(),
        queue_group: Some("from-args".to_string()),
    };
    let (config, _) = config_from_args(args, &env).expect("valid args");
    // CLI value wins over env so operators can override at launch.
    assert_eq!(config.queue_group.as_deref(), Some("from-args"));
}

#[test]
fn parse_servers_splits_and_trims_csv() {
    assert_eq!(parse_servers("a,b,c"), vec!["a", "b", "c"]);
    assert_eq!(parse_servers(" a , b "), vec!["a", "b"]);
    assert!(parse_servers("").is_empty());
    assert!(parse_servers(",,").is_empty());
}

#[test]
fn config_carries_servers_from_nats_url() {
    let env = InMemoryEnv::new();
    let args = Args {
        nats_url: "nats-a:4222,nats-b:4222".to_string(),
        prefix: "a2a".to_string(),
        queue_group: None,
    };
    let (config, nats) = config_from_args(args, &env).expect("valid args");
    assert_eq!(config.nats_servers, vec!["nats-a:4222", "nats-b:4222"]);
    assert_eq!(nats.servers, config.nats_servers);
}
