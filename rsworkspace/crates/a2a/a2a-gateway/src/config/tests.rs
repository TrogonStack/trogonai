use trogon_std::env::InMemoryEnv;

use super::*;

fn args(nats_url: &str, prefix: &str, queue_group: Option<&str>) -> Args {
    Args {
        nats_url: nats_url.to_string(),
        prefix: prefix.to_string(),
        queue_group: queue_group.map(ToOwned::to_owned),
    }
}

#[test]
fn config_from_args_rejects_invalid_prefix() {
    let env = InMemoryEnv::new();
    let result = config_from_args(args("localhost:4222", "bad prefix!", None), &env);
    assert!(matches!(result, Err(ConfigError::InvalidPrefix(_))));
}

#[test]
fn config_gateway_subscribe_subject_uses_prefix() {
    let env = InMemoryEnv::new();
    let (config, _) = config_from_args(args("localhost:4222", "a2a", Some("gateways")), &env).expect("valid args");
    assert_eq!(config.gateway_subscribe_subject(), "a2a.gateway.>");
    assert_eq!(config.queue_group.as_ref().map(QueueGroup::as_str), Some("gateways"));
}

#[test]
fn config_from_args_falls_back_to_env_queue_group() {
    let env = InMemoryEnv::new();
    env.set("A2A_GATEWAY_QUEUE_GROUP", "from-env");
    let (config, _) = config_from_args(args("localhost:4222", "a2a", None), &env).expect("valid args");
    assert_eq!(config.queue_group.as_ref().map(QueueGroup::as_str), Some("from-env"));
}

#[test]
fn config_from_args_args_override_env_queue_group() {
    let env = InMemoryEnv::new();
    env.set("A2A_GATEWAY_QUEUE_GROUP", "from-env");
    let (config, _) = config_from_args(args("localhost:4222", "a2a", Some("from-args")), &env).expect("valid args");
    // CLI value wins over env so operators can override at launch.
    assert_eq!(config.queue_group.as_ref().map(QueueGroup::as_str), Some("from-args"));
}

#[test]
fn config_from_args_rejects_empty_queue_group_from_cli() {
    let env = InMemoryEnv::new();
    let err = config_from_args(args("localhost:4222", "a2a", Some("   ")), &env).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidQueueGroup(QueueGroupError::Empty)));
}

#[test]
fn config_from_args_rejects_empty_queue_group_from_env() {
    let env = InMemoryEnv::new();
    env.set("A2A_GATEWAY_QUEUE_GROUP", "  ");
    let err = config_from_args(args("localhost:4222", "a2a", None), &env).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidQueueGroup(QueueGroupError::Empty)));
}

#[test]
fn parse_servers_splits_and_trims_csv() {
    let servers = parse_servers("a,b,c").expect("non-empty");
    assert_eq!(
        servers.iter().map(NatsServerUrl::as_str).collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
    let servers = parse_servers(" a , b ").expect("non-empty");
    assert_eq!(
        servers.iter().map(NatsServerUrl::as_str).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

#[test]
fn parse_servers_rejects_empty_or_whitespace_only_input() {
    assert!(matches!(parse_servers(""), Err(ConfigError::EmptyNatsServers { .. })));
    assert!(matches!(parse_servers(",,"), Err(ConfigError::EmptyNatsServers { .. })));
    assert!(matches!(
        parse_servers("   "),
        Err(ConfigError::EmptyNatsServers { .. })
    ));
}

#[test]
fn config_from_args_rejects_empty_nats_url() {
    let env = InMemoryEnv::new();
    let err = config_from_args(args("", "a2a", None), &env).unwrap_err();
    assert!(matches!(err, ConfigError::EmptyNatsServers { .. }));
}

#[test]
fn config_carries_servers_from_nats_url() {
    let env = InMemoryEnv::new();
    let (config, nats) = config_from_args(args("nats-a:4222,nats-b:4222", "a2a", None), &env).expect("valid args");
    let server_strs: Vec<&str> = config.nats_servers.iter().map(NatsServerUrl::as_str).collect();
    assert_eq!(server_strs, vec!["nats-a:4222", "nats-b:4222"]);
    assert_eq!(
        nats.servers,
        server_strs.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn nats_server_url_rejects_empty() {
    assert!(matches!(NatsServerUrl::new(""), Err(NatsServerUrlError::Empty)));
    assert!(matches!(NatsServerUrl::new("  "), Err(NatsServerUrlError::Empty)));
}

#[test]
fn nats_server_url_trims_and_displays() {
    let url = NatsServerUrl::new("  nats:4222  ").expect("trim accepts");
    assert_eq!(url.as_str(), "nats:4222");
    assert_eq!(format!("{url}"), "nats:4222");
}

#[test]
fn queue_group_rejects_empty() {
    assert!(matches!(QueueGroup::new(""), Err(QueueGroupError::Empty)));
    assert!(matches!(QueueGroup::new("  "), Err(QueueGroupError::Empty)));
}

#[test]
fn queue_group_trims() {
    let qg = QueueGroup::new("  gateways  ").expect("trim accepts");
    assert_eq!(qg.as_str(), "gateways");
}
