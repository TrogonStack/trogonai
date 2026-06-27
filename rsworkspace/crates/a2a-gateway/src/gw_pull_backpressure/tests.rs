use trogon_std::env::InMemoryEnv;

use super::*;

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a".to_string()).expect("test prefix")
}

#[test]
fn durable_name_scoped_to_prefix() {
    assert_eq!(
        EventsConsumerDurable::for_prefix(&prefix()).as_str(),
        "A2A_GATEWAY_EVENTS"
    );
}

#[test]
fn durable_name_uppercases_and_underscores_dots() {
    let prefix = A2aPrefix::new("tenant.acme".to_string()).expect("dotted prefix");
    assert_eq!(
        EventsConsumerDurable::for_prefix(&prefix).as_str(),
        "TENANT_ACME_GATEWAY_EVENTS"
    );
}

#[test]
fn max_ack_pending_floors_at_one() {
    assert_eq!(GatewayEventsMaxAckPending::new(0).as_usize(), 1);
    assert_eq!(GatewayEventsMaxAckPending::new(0).as_i64(), 1);
    assert_eq!(GatewayEventsMaxAckPending::new(5).as_usize(), 5);
}

#[test]
fn fetch_batch_floors_at_one() {
    assert_eq!(GatewayEventsFetchBatch::new(0).as_usize(), 1);
    assert_eq!(GatewayEventsFetchBatch::new(3).as_usize(), 3);
}

#[test]
fn max_inflight_per_caller_floors_at_one() {
    assert_eq!(GatewayEventsMaxInflightPerCaller::new(0).as_usize(), 1);
    assert_eq!(GatewayEventsMaxInflightPerCaller::new(8).as_usize(), 8);
}

#[test]
fn pull_consumer_hints_baseline_uses_documented_defaults() {
    let hints = PullConsumerHints::gateway_baseline();
    assert_eq!(hints.max_ack_pending, DEFAULT_MAX_ACK_PENDING);
    assert_eq!(hints.inactive_threshold_secs, DEFAULT_INACTIVE_THRESHOLD_SECS);
    assert_eq!(hints.ack_policy, AckPolicy::Explicit);
    assert_eq!(
        hints.inactive_threshold(),
        Duration::from_secs(DEFAULT_INACTIVE_THRESHOLD_SECS)
    );
}

#[test]
fn pull_consumer_hints_default_matches_baseline() {
    assert_eq!(PullConsumerHints::default(), PullConsumerHints::gateway_baseline());
}

#[test]
fn parse_task_events_subject_extracts_ids() {
    // JetStream task event subjects use the plural `tasks` segment, not
    // `task` — regression test for the wrong-prefix bug.
    let (task_id, req_id) = parse_task_events_subject("a2a", "a2a.tasks.t1.events.r1").expect("parsed");
    assert_eq!(task_id.as_str(), "t1");
    assert_eq!(req_id.as_str(), "r1");
}

#[test]
fn parse_task_events_subject_rejects_singular_task_segment() {
    // Anti-regression: the wrong prefix Term'd every pulled message.
    assert!(parse_task_events_subject("a2a", "a2a.task.t1.events.r1").is_none());
}

#[test]
fn parse_task_events_subject_rejects_unrelated() {
    assert!(parse_task_events_subject("a2a", "a2a.gateway.bot.message.send").is_none());
    assert!(parse_task_events_subject("a2a", "other.tasks.t1.events.r1").is_none());
    // Missing the `.events.` infix.
    assert!(parse_task_events_subject("a2a", "a2a.tasks.t1.r1").is_none());
}

#[test]
fn planner_forward_disposition_retries_then_terms() {
    let planner = BaselineTaskEventsEgressPlanner::new();
    assert_eq!(
        planner.forward_disposition(1, Some("fail")),
        EgressAckDisposition::Nak { delay: None }
    );
    assert_eq!(planner.forward_disposition(3, Some("fail")), EgressAckDisposition::Term);
    assert_eq!(planner.forward_disposition(1, None), EgressAckDisposition::Ack);
}

#[test]
fn planner_message_stream_plan_filters_by_req_id() {
    let planner = BaselineTaskEventsEgressPlanner::new();
    let plan = planner
        .plan_message_stream(&prefix(), &ReqId::from_header("req-1"))
        .expect("plan");
    assert_eq!(plan.filter_subject, "a2a.tasks.*.events.req-1");
}

#[test]
fn planner_resubscribe_plan_filters_by_task_id_and_advances_seq() {
    let planner = BaselineTaskEventsEgressPlanner::new();
    let task_id = A2aTaskId::new("task-9").expect("task");
    let plan = planner.plan_resubscribe(&prefix(), &task_id, 41).expect("plan");
    assert_eq!(plan.filter_subject, "a2a.tasks.task-9.events.*");
    assert_eq!(plan.start_sequence, 42);
}

#[test]
fn planner_resubscribe_plan_handles_zero_last_seq() {
    let planner = BaselineTaskEventsEgressPlanner::new();
    let task_id = A2aTaskId::new("task-9").expect("task");
    let plan = planner.plan_resubscribe(&prefix(), &task_id, 0).expect("plan");
    assert_eq!(plan.start_sequence, 1);
}

#[test]
fn gateway_events_pull_enabled_reads_on_flag() {
    let env = InMemoryEnv::new();
    assert!(!gateway_events_pull_enabled(&env));
    env.set(ENV_GATEWAY_EVENTS_PULL, "on");
    assert!(gateway_events_pull_enabled(&env));
}

#[test]
fn config_from_env_applies_overrides() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_EVENTS_MAX_ACK_PENDING, "512");
    env.set(ENV_GATEWAY_EVENTS_FETCH_BATCH, "4");
    env.set(ENV_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS, "10");
    env.set(ENV_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER, "8");
    let cfg = GatewayEventsPullConfig::from_env(&env);
    assert_eq!(cfg.max_ack_pending().as_usize(), 512);
    assert_eq!(cfg.fetch_batch().as_usize(), 4);
    assert_eq!(cfg.fetch_heartbeat().as_duration(), Duration::from_secs(10));
    assert_eq!(cfg.max_inflight_per_caller().as_usize(), 8);
}

#[test]
fn config_from_env_uses_documented_defaults() {
    let env = InMemoryEnv::new();
    let cfg = GatewayEventsPullConfig::from_env(&env);
    assert_eq!(cfg.max_ack_pending().as_usize(), DEFAULT_MAX_ACK_PENDING);
    assert_eq!(cfg.fetch_batch().as_usize(), DEFAULT_FETCH_BATCH);
    assert_eq!(
        cfg.fetch_heartbeat().as_duration(),
        Duration::from_secs(DEFAULT_FETCH_HEARTBEAT_SECS)
    );
    assert_eq!(
        cfg.max_inflight_per_caller().as_usize(),
        DEFAULT_MAX_INFLIGHT_PER_CALLER
    );
}

#[test]
fn config_from_env_clamps_zero_heartbeat() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS, "0");
    let cfg = GatewayEventsPullConfig::from_env(&env);
    // Zero would mean "no heartbeat" — JetStream needs a positive value.
    assert_eq!(cfg.fetch_heartbeat().as_duration(), Duration::from_secs(1));
}

#[test]
fn config_from_env_clamps_zero_max_inflight() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER, "0");
    let cfg = GatewayEventsPullConfig::from_env(&env);
    // Zero would NAK every message into a tight loop.
    assert_eq!(cfg.max_inflight_per_caller().as_usize(), 1);
}

#[test]
fn gateway_egress_subject_includes_req_id() {
    let req_id = ReqId::from_header("req-1");
    assert_eq!(gateway_egress_subject(&prefix(), &req_id), "a2a.gateway.egress.req-1");
}

#[test]
fn caller_inflight_gate_enforces_limit() {
    let gate = std::sync::Arc::new(CallerInflightGate::new(1));
    let first = gate.clone().try_acquire("caller-a");
    assert!(first.is_some());
    assert!(gate.clone().try_acquire("caller-a").is_none());
    drop(first);
    assert!(gate.clone().try_acquire("caller-a").is_some());
}

#[test]
fn caller_inflight_gate_tracks_callers_independently() {
    let gate = std::sync::Arc::new(CallerInflightGate::new(1));
    let _a = gate.clone().try_acquire("caller-a").expect("a");
    let _b = gate.clone().try_acquire("caller-b").expect("b");
    assert!(gate.clone().try_acquire("caller-a").is_none());
    assert!(gate.clone().try_acquire("caller-b").is_none());
}

#[test]
fn pull_hints_planner_returns_baseline() {
    let hints = BaselineTaskEventsEgressPlanner::new().pull_hints();
    assert_eq!(hints, PullConsumerHints::gateway_baseline());
}

#[test]
fn pull_config_new_round_trips_value_objects() {
    let cfg = GatewayEventsPullConfig::new(
        GatewayEventsMaxAckPending::new(99),
        GatewayEventsFetchBatch::new(7),
        GatewayEventsFetchHeartbeat::new(Duration::from_secs(11)),
        GatewayEventsMaxInflightPerCaller::new(3),
    );
    assert_eq!(cfg.max_ack_pending().as_usize(), 99);
    assert_eq!(cfg.fetch_batch().as_usize(), 7);
    assert_eq!(cfg.fetch_heartbeat().as_duration(), Duration::from_secs(11));
    assert_eq!(cfg.max_inflight_per_caller().as_usize(), 3);
}

#[test]
fn fetch_heartbeat_floors_at_one_second() {
    assert_eq!(
        GatewayEventsFetchHeartbeat::new(Duration::ZERO).as_duration(),
        Duration::from_secs(1)
    );
    assert_eq!(
        GatewayEventsFetchHeartbeat::new(Duration::from_secs(5)).as_duration(),
        Duration::from_secs(5)
    );
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
    // The poison-recovery branch returns the inner guard so the gate keeps
    // serving instead of cascading the panic to every caller.
    let permit = gate.clone().try_acquire("caller-a").expect("recovers from poison");
    drop(permit);
}

#[test]
fn pull_cycle_error_display_carries_subject() {
    let err = PullCycleError::Ack {
        subject: "a2a.tasks.t.events.r".to_string(),
        source: "boom".into(),
    };
    assert!(format!("{err}").contains("a2a.tasks.t.events.r"));
}
