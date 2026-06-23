use super::*;
use crate::constants::{
    DEFAULT_PUSH_DLQ_CALLER_SEGMENT as PLACEHOLDER_CALLER_SEGMENT, ENV_MAX_CONCURRENT_CLIENT_TASKS,
    ENV_PUSH_DLQ_CALLER_SEGMENT,
};

fn default_nats() -> NatsConfig {
    NatsConfig {
        servers: vec!["localhost:4222".to_string()],
        auth: trogon_nats::NatsAuth::None,
    }
}

fn with_subscriber<F: FnOnce()>(f: F) {
    use tracing_subscriber::util::SubscriberInitExt;
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();
    f();
}

#[test]
fn config_new_accepts_validated_prefix() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats());
    assert_eq!(config.a2a_prefix(), "a2a");
}

#[test]
fn config_with_operation_timeout() {
    let config =
        Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_operation_timeout(Duration::from_secs(45));
    assert_eq!(config.operation_timeout(), Duration::from_secs(45));
}

#[test]
fn config_with_task_timeout() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_task_timeout(Duration::from_secs(60));
    assert_eq!(config.task_timeout(), Duration::from_secs(60));
}

#[test]
fn config_with_max_concurrent_client_tasks_enforces_minimum() {
    let config = Config::for_test("a2a").with_max_concurrent_client_tasks(0);
    assert_eq!(config.max_concurrent_client_tasks(), 1);
}

#[test]
fn config_default_max_concurrent_client_tasks() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats());
    assert_eq!(
        config.max_concurrent_client_tasks(),
        DEFAULT_MAX_CONCURRENT_CLIENT_TASKS
    );
}

#[test]
fn config_push_dlq_caller_segment_trim_empty_falls_back() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_push_dlq_caller_segment("");
    assert_eq!(config.push_dlq_caller_segment().as_str(), PLACEHOLDER_CALLER_SEGMENT);
}

#[test]
fn config_push_dlq_caller_segment_preserves_explicit_value() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_push_dlq_caller_segment("alice");
    assert_eq!(config.push_dlq_caller_segment().as_str(), "alice");
}

#[test]
fn config_nats_returns_nats_config() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats());
    assert_eq!(config.nats().servers.len(), 1);
}

#[test]
fn nats_connect_timeout_defaults() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        assert_eq!(
            nats_connect_timeout(&env),
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
        );
    });
}

#[test]
fn nats_connect_timeout_valid_override() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_CONNECT_TIMEOUT_SECS, "15");
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(15));
    });
}

#[test]
fn nats_connect_timeout_below_minimum_uses_default() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_CONNECT_TIMEOUT_SECS, "0");
        assert_eq!(
            nats_connect_timeout(&env),
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
        );
    });
}

#[test]
fn nats_connect_timeout_invalid_uses_default() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_CONNECT_TIMEOUT_SECS, "bogus");
        assert_eq!(
            nats_connect_timeout(&env),
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
        );
    });
}

#[test]
fn operation_timeout_invalid_env_is_ignored() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        let default_timeout = Config::for_test("a2a").operation_timeout();
        env.set(ENV_OPERATION_TIMEOUT_SECS, "bogus");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.operation_timeout(), default_timeout);
    });
}

#[test]
fn operation_timeout_below_min_is_ignored() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        let default_timeout = Config::for_test("a2a").operation_timeout();
        env.set(ENV_OPERATION_TIMEOUT_SECS, "0");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.operation_timeout(), default_timeout);
    });
}

#[test]
fn operation_timeout_valid_override() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_OPERATION_TIMEOUT_SECS, "45");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.operation_timeout(), Duration::from_secs(45));
    });
}

#[test]
fn task_timeout_invalid_env_is_ignored() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        let default_timeout = Config::for_test("a2a").task_timeout();
        env.set(ENV_TASK_TIMEOUT_SECS, "bogus");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.task_timeout(), default_timeout);
    });
}

#[test]
fn task_timeout_below_min_is_ignored() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        let default_timeout = Config::for_test("a2a").task_timeout();
        env.set(ENV_TASK_TIMEOUT_SECS, "0");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.task_timeout(), default_timeout);
    });
}

#[test]
fn task_timeout_valid_override() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_TASK_TIMEOUT_SECS, "120");
        let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
        assert_eq!(cfg.task_timeout(), Duration::from_secs(120));
    });
}

#[test]
fn push_dlq_caller_segment_env_override() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_PUSH_DLQ_CALLER_SEGMENT, "oidc-sub-7");
    let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
    assert_eq!(cfg.push_dlq_caller_segment().as_str(), "oidc-sub-7");
}

#[test]
fn push_dlq_caller_segment_blank_env_falls_back_via_builder() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_PUSH_DLQ_CALLER_SEGMENT, "   ");
    let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
    assert_eq!(cfg.push_dlq_caller_segment().as_str(), PLACEHOLDER_CALLER_SEGMENT);
}

#[test]
fn max_concurrent_client_tasks_env_override() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_MAX_CONCURRENT_CLIENT_TASKS, "32");
    let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
    assert_eq!(cfg.max_concurrent_client_tasks(), 32);
}

#[test]
fn max_concurrent_client_tasks_env_zero_normalized_to_one() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_MAX_CONCURRENT_CLIENT_TASKS, "0");
    let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
    assert_eq!(cfg.max_concurrent_client_tasks(), 1);
}

#[test]
fn max_concurrent_client_tasks_invalid_env_is_ignored() {
    let env = trogon_std::env::InMemoryEnv::new();
    let default_max = Config::for_test("a2a").max_concurrent_client_tasks();
    env.set(ENV_MAX_CONCURRENT_CLIENT_TASKS, "bogus");
    let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
    assert_eq!(cfg.max_concurrent_client_tasks(), default_max);
}

#[test]
fn a2a_prefix_ref_returns_prefix_value_object() {
    let prefix = A2aPrefix::new("a2a-test").unwrap();
    let config = Config::new(prefix.clone(), default_nats());
    assert_eq!(config.a2a_prefix_ref().as_str(), prefix.as_str());
}

#[test]
fn push_dlq_dedup_lru_size_default() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats());
    assert_eq!(config.push_dlq_dedup_lru_size(), DEFAULT_PUSH_DLQ_DEDUP_LRU_SIZE);
}

#[test]
fn with_operation_timeout_clamps_below_minimum_to_minimum() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_operation_timeout(Duration::ZERO);
    assert_eq!(config.operation_timeout(), Duration::from_secs(MIN_TIMEOUT_SECS));
}

#[test]
fn with_task_timeout_clamps_below_minimum_to_minimum() {
    let config = Config::new(A2aPrefix::new("a2a").unwrap(), default_nats()).with_task_timeout(Duration::ZERO);
    assert_eq!(config.task_timeout(), Duration::from_secs(MIN_TIMEOUT_SECS));
}

#[test]
fn push_dlq_dedup_lru_size_env_override() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_PUSH_DLQ_DEDUP_LRU_SIZE, "4096");
    let cfg = apply_timeout_overrides(Config::for_test("a2a"), &env);
    assert_eq!(cfg.push_dlq_dedup_lru_size(), 4096);
}

#[test]
fn push_dlq_dedup_lru_size_zero_preserves_in_code_value() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_PUSH_DLQ_DEDUP_LRU_SIZE, "0");
    with_subscriber(|| {
        let mut base = Config::for_test("a2a");
        base.push_dlq_dedup_lru_size = 9999;
        let cfg = apply_timeout_overrides(base, &env);
        assert_eq!(cfg.push_dlq_dedup_lru_size(), 9999);
    });
}

#[test]
fn push_dlq_dedup_lru_size_invalid_preserves_in_code_value() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_PUSH_DLQ_DEDUP_LRU_SIZE, "not-a-number");
    with_subscriber(|| {
        let mut base = Config::for_test("a2a");
        base.push_dlq_dedup_lru_size = 9999;
        let cfg = apply_timeout_overrides(base, &env);
        assert_eq!(cfg.push_dlq_dedup_lru_size(), 9999);
    });
}
