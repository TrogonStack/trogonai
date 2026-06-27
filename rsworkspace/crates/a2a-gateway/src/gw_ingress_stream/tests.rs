use trogon_std::env::InMemoryEnv;

use super::*;

#[test]
fn resubscribe_params_parse_full_shape() {
    let params = serde_json::json!({"id": "task-1", "last_seq": 41});
    let (task_id, last_seq) = parse_resubscribe_params(&params).expect("parsed");
    assert_eq!(task_id.as_str(), "task-1");
    assert_eq!(last_seq, 41);
}

#[test]
fn resubscribe_params_accept_task_id_aliases() {
    for key in ["id", "task_id", "taskId"] {
        let params = serde_json::json!({ key: "task-1" });
        let (task_id, last_seq) = parse_resubscribe_params(&params).expect("alias parses");
        assert_eq!(task_id.as_str(), "task-1");
        assert_eq!(last_seq, 0, "last_seq defaults to 0 when absent");
    }
}

#[test]
fn resubscribe_params_accept_camel_last_seq() {
    let params = serde_json::json!({"id": "task-1", "lastSeq": 9});
    let (_task_id, last_seq) = parse_resubscribe_params(&params).expect("camel parses");
    assert_eq!(last_seq, 9);
}

#[test]
fn resubscribe_params_missing_id_is_typed_error() {
    let params = serde_json::json!({});
    let err = parse_resubscribe_params(&params).unwrap_err();
    assert!(matches!(err, ResubscribeParamsError::MissingTaskId));
}

#[test]
fn resubscribe_params_invalid_task_id_surfaces_validator_error() {
    // A2aTaskId rejects whitespace.
    let params = serde_json::json!({"id": " "});
    let err = parse_resubscribe_params(&params).unwrap_err();
    assert!(matches!(err, ResubscribeParamsError::InvalidTaskId(_)));
}

#[test]
fn resubscribe_params_garbage_payload_is_deserialize_error() {
    // `id` must be a string; pass a number to force deserialize failure.
    let params = serde_json::json!({"id": 12345});
    let err = parse_resubscribe_params(&params).unwrap_err();
    assert!(matches!(err, ResubscribeParamsError::Deserialize(_)));
}

#[test]
fn caller_key_rejects_empty() {
    assert!(matches!(CallerKey::new(""), Err(CallerKeyError::Empty)));
    assert!(matches!(CallerKey::new("   "), Err(CallerKeyError::Empty)));
}

#[test]
fn caller_key_trims_whitespace() {
    let key = CallerKey::new("  bot  ").expect("trims");
    assert_eq!(key.as_str(), "bot");
}

#[test]
fn streaming_max_ack_pending_floors_at_one() {
    assert_eq!(StreamingMaxAckPending::new(0).as_i64(), 1);
    assert_eq!(StreamingMaxAckPending::new(-5).as_i64(), 1);
    assert_eq!(StreamingMaxAckPending::new(32).as_i64(), 32);
}

#[test]
fn streaming_max_inflight_floors_at_one() {
    assert_eq!(StreamingMaxInflightPerCaller::new(0).as_usize(), 1);
    assert_eq!(StreamingMaxInflightPerCaller::new(8).as_usize(), 8);
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
fn config_from_env_applies_overrides() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_STREAMING_MAX_ACK_PENDING, "64");
    env.set(ENV_GATEWAY_STREAMING_MAX_INFLIGHT, "8");
    let cfg = GatewayStreamingIngressConfig::from_env(&env);
    assert_eq!(cfg.max_ack_pending().as_i64(), 64);
    assert_eq!(cfg.max_inflight_per_caller().as_usize(), 8);
}

#[test]
fn config_from_env_uses_defaults() {
    let env = InMemoryEnv::new();
    let cfg = GatewayStreamingIngressConfig::from_env(&env);
    assert_eq!(cfg.max_ack_pending().as_i64(), DEFAULT_STREAMING_MAX_ACK_PENDING);
    assert_eq!(cfg.max_inflight_per_caller().as_usize(), DEFAULT_STREAMING_MAX_INFLIGHT);
}

#[test]
fn config_from_env_clamps_zero_to_one() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_STREAMING_MAX_ACK_PENDING, "0");
    env.set(ENV_GATEWAY_STREAMING_MAX_INFLIGHT, "0");
    let cfg = GatewayStreamingIngressConfig::from_env(&env);
    assert_eq!(cfg.max_ack_pending().as_i64(), 1);
    assert_eq!(cfg.max_inflight_per_caller().as_usize(), 1);
}

#[test]
fn caller_inflight_gate_enforces_limit_and_releases_on_drop() {
    let gate = std::sync::Arc::new(CallerInflightGate::new(2));
    let caller = CallerKey::new("caller-a").unwrap();
    let first = gate.clone().try_acquire(&caller).expect("first");
    let second = gate.clone().try_acquire(&caller).expect("second");
    assert!(gate.clone().try_acquire(&caller).is_none(), "limit hit");
    drop(first);
    assert!(gate.clone().try_acquire(&caller).is_some(), "permit returned");
    drop(second);
}

#[test]
fn streaming_ingress_gate_shares_state_across_clones() {
    // Two clones of the gate (the runtime hands the same Arc to every
    // spawn) must enforce the same per-caller limit — a naive per-pump
    // gate would let each pump grant itself a fresh permit.
    let cfg =
        GatewayStreamingIngressConfig::new(StreamingMaxAckPending::new(32), StreamingMaxInflightPerCaller::new(1));
    let gate = StreamingIngressGate::new(cfg);
    let caller = CallerKey::new("caller-a").unwrap();
    let permit = gate.try_acquire(&caller).expect("first");
    // Cloning the gate (Arc-clone) does not give the caller a fresh bucket.
    assert!(gate.clone().try_acquire(&caller).is_none());
    drop(permit);
    assert!(gate.try_acquire(&caller).is_some());
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

#[test]
fn streaming_ingress_kind_resubscribe_requires_task_id_by_type() {
    // Compile-only check: the variant has `task_id` as a non-optional
    // field, so a typo or missing-field bug surfaces at the call site
    // rather than as a runtime warning + silent drop.
    let task = A2aTaskId::new("t-1").unwrap();
    let kind = StreamingIngressKind::TasksResubscribe {
        req_id: ReqId::from_header("r"),
        task_id: task,
        last_seq: 0,
    };
    assert!(matches!(kind, StreamingIngressKind::TasksResubscribe { .. }));
}

#[test]
fn streaming_ingress_spawn_error_carries_caller() {
    let err = StreamingIngressSpawnError::PerCallerLimit { caller: "bot".into() };
    assert!(format!("{err}").contains("bot"));
}

#[test]
fn caller_inflight_gate_recovers_from_poisoned_lock() {
    let gate = std::sync::Arc::new(CallerInflightGate::new(2));
    let gate_clone = std::sync::Arc::clone(&gate);
    let _ = std::thread::spawn(move || {
        let _guard = gate_clone.inflight.lock().unwrap();
        panic!("poison the lock on purpose");
    })
    .join();
    let caller = CallerKey::new("caller-a").unwrap();
    // The poison-recovery branch returns the inner guard so the gate keeps
    // serving instead of cascading the panic to every caller.
    let permit = gate.try_acquire(&caller).expect("recovers from poison");
    drop(permit);
}

#[test]
fn try_acquire_streaming_permit_passes_when_capacity_available() {
    let cfg =
        GatewayStreamingIngressConfig::new(StreamingMaxAckPending::new(16), StreamingMaxInflightPerCaller::new(2));
    let gate = StreamingIngressGate::new(cfg);
    let spawn = StreamingIngressSpawn {
        kind: StreamingIngressKind::MessageStream {
            req_id: ReqId::from_header("r-1"),
        },
        reply: async_nats::Subject::from_static("reply.x"),
        caller_key: CallerKey::new("bot").unwrap(),
    };
    let permit = try_acquire_streaming_permit(&gate, &spawn).expect("first permit");
    drop(permit);
}

#[test]
fn try_acquire_streaming_permit_returns_per_caller_limit_when_full() {
    let cfg =
        GatewayStreamingIngressConfig::new(StreamingMaxAckPending::new(16), StreamingMaxInflightPerCaller::new(1));
    let gate = StreamingIngressGate::new(cfg);
    let spawn = StreamingIngressSpawn {
        kind: StreamingIngressKind::MessageStream {
            req_id: ReqId::from_header("r-1"),
        },
        reply: async_nats::Subject::from_static("reply.x"),
        caller_key: CallerKey::new("bot").unwrap(),
    };
    let _held = try_acquire_streaming_permit(&gate, &spawn).expect("first");
    let err = try_acquire_streaming_permit(&gate, &spawn).expect_err("second");
    assert!(matches!(err, StreamingIngressSpawnError::PerCallerLimit { ref caller } if caller == "bot"));
}

#[tokio::test]
async fn publish_to_caller_reply_routes_payload_to_reply_subject() {
    let mock = trogon_nats::AdvancedMockNatsClient::new();
    let reply = async_nats::Subject::from_static("inbox.r");
    let payload = bytes::Bytes::from_static(b"chunk");
    publish_to_caller_reply(&mock, &reply, payload.clone())
        .await
        .expect("publish ok");
    assert_eq!(mock.published_messages(), vec!["inbox.r"]);
    assert_eq!(mock.published_payloads(), vec![payload]);
}

#[tokio::test]
async fn publish_to_caller_reply_surfaces_publish_error() {
    let mock = trogon_nats::AdvancedMockNatsClient::new();
    mock.fail_next_publish();
    publish_to_caller_reply(&mock, &async_nats::Subject::from_static("x"), bytes::Bytes::new())
        .await
        .expect_err("publish fails");
}

#[test]
fn caller_inflight_gate_drop_removes_empty_bucket() {
    // Once a caller drops back to zero permits, the bucket key is removed —
    // important to avoid an unbounded HashMap grow under churn.
    let gate = std::sync::Arc::new(CallerInflightGate::new(1));
    let caller = CallerKey::new("bot").unwrap();
    let permit = gate.clone().try_acquire(&caller).expect("permit");
    assert!(gate.inflight.lock().unwrap().contains_key("bot"));
    drop(permit);
    assert!(!gate.inflight.lock().unwrap().contains_key("bot"));
}
