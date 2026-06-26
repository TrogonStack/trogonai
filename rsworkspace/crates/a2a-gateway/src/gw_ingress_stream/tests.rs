use trogon_std::env::InMemoryEnv;

use super::*;

#[test]
fn resubscribe_params_extract_task_and_seq() {
    let params = serde_json::json!({"id": "task-1", "last_seq": 41});
    let task_id = task_id_from_resubscribe_params(&params).expect("task");
    assert_eq!(task_id.as_str(), "task-1");
    assert_eq!(last_seq_from_resubscribe_params(&params), Some(41));
}

#[test]
fn resubscribe_params_accept_task_id_aliases() {
    // Wire flexibility — three accepted shapes for the task id.
    for key in ["id", "task_id", "taskId"] {
        let params = serde_json::json!({ key: "task-1" });
        let task = task_id_from_resubscribe_params(&params).expect("task id alias");
        assert_eq!(task.as_str(), "task-1");
    }
    let snake = serde_json::json!({"last_seq": 10});
    let camel = serde_json::json!({"lastSeq": 10});
    assert_eq!(last_seq_from_resubscribe_params(&snake), Some(10));
    assert_eq!(last_seq_from_resubscribe_params(&camel), Some(10));
}

#[test]
fn resubscribe_params_return_none_when_missing() {
    let params = serde_json::json!({});
    assert!(task_id_from_resubscribe_params(&params).is_none());
    assert!(last_seq_from_resubscribe_params(&params).is_none());
}

#[test]
fn streaming_ingress_enabled_reads_flag() {
    let env = InMemoryEnv::new();
    assert!(!gateway_streaming_ingress_enabled(&env));
    env.set(ENV_GATEWAY_STREAMING_INGRESS, "on");
    assert!(gateway_streaming_ingress_enabled(&env));
    env.set(ENV_GATEWAY_STREAMING_INGRESS, "false");
    assert!(!gateway_streaming_ingress_enabled(&env));
}

#[test]
fn config_from_env_applies_overrides_and_clamps_to_one() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_STREAMING_MAX_ACK_PENDING, "64");
    env.set(ENV_GATEWAY_STREAMING_MAX_INFLIGHT, "8");
    let cfg = GatewayStreamingIngressConfig::from_env(&env);
    assert_eq!(cfg.max_ack_pending, 64);
    assert_eq!(cfg.max_inflight_per_caller, 8);
}

#[test]
fn config_from_env_uses_defaults_when_unset() {
    let env = InMemoryEnv::new();
    let cfg = GatewayStreamingIngressConfig::from_env(&env);
    assert_eq!(cfg.max_ack_pending, DEFAULT_STREAMING_MAX_ACK_PENDING);
    assert_eq!(cfg.max_inflight_per_caller, DEFAULT_STREAMING_MAX_INFLIGHT);
}

#[test]
fn config_from_env_clamps_zero_to_one() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_STREAMING_MAX_ACK_PENDING, "0");
    env.set(ENV_GATEWAY_STREAMING_MAX_INFLIGHT, "0");
    let cfg = GatewayStreamingIngressConfig::from_env(&env);
    // Floor at 1: a zero would stall the consumer with no inflight permits.
    assert_eq!(cfg.max_ack_pending, 1);
    assert_eq!(cfg.max_inflight_per_caller, 1);
}

#[test]
fn caller_inflight_gate_enforces_limit_and_releases_on_drop() {
    let gate = CallerInflightGate::new(2);
    let first = gate.try_acquire("caller-a").expect("first");
    let second = gate.try_acquire("caller-a").expect("second");
    assert!(gate.try_acquire("caller-a").is_none(), "limit hit");
    drop(first);
    assert!(gate.try_acquire("caller-a").is_some(), "permit returned");
    drop(second);
}

#[test]
fn req_id_from_headers_reads_header_first() {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(a2a_nats::constants::REQ_ID_HEADER, "req-from-header");
    let req_id = req_id_from_headers_or_payload(&headers, b"{}").expect("header wins");
    assert_eq!(req_id.as_str(), "req-from-header");
}

#[test]
fn req_id_from_payload_when_header_absent() {
    let headers = async_nats::HeaderMap::new();
    let payload = br#"{"jsonrpc":"2.0","id":"req-from-payload","method":"x"}"#;
    let req_id = req_id_from_headers_or_payload(&headers, payload).expect("payload extract");
    assert_eq!(req_id.as_str(), "req-from-payload");
}
