use std::path::{Path, PathBuf};

use a2a_auth_callout::SpiceDbSubject;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use a2a_gateway::policy::tier1_declarative::{
    FixedTier1Clock, RealTier1DeclarativeGate, Tier1DeclarativeBundle, Tier1DeclarativeContext,
    Tier1DeclarativeDecision, Tier1DeclarativeGate, Tier1DeclarativeRuleId, tier1_declarative_audit_rule_fired,
};
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::ingress_gateway_policy_denied_response_bytes;
use a2a_nats::A2aMethod;

fn reference_policies_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../a2a-pack/policies")
}

fn load_reference_bundle(filename: &str) -> Tier1DeclarativeBundle {
    let src = reference_policies_dir().join(filename);
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::copy(&src, dir.path().join(filename)).expect("copy reference bundle");
    Tier1DeclarativeBundle::load_from_dir(dir.path()).expect("load bundle")
}

fn ctx(
    method: A2aMethod,
    agent: &str,
    caller_subject: Option<&str>,
    nats_subject: &str,
) -> Tier1DeclarativeContext {
    Tier1DeclarativeContext::new(
        method,
        A2aAgentId::new(agent).expect("agent id"),
        caller_subject.map(SpiceDbSubject::new),
        nats_subject,
    )
}

fn ingress_subject(agent: &str, method_dots: &str) -> String {
    format!("a2a.gateway.{agent}.{method_dots}")
}

#[test]
fn per_method_allowlist_reference_bundle_loads() {
    let bundle = load_reference_bundle("per-method-allowlist.tier1.toml");
    assert_eq!(bundle.rules().len(), 4);
}

#[test]
fn per_method_allowlist_allows_enumerated_methods() {
    let gate = RealTier1DeclarativeGate::new(load_reference_bundle("per-method-allowlist.tier1.toml"));
    let subject = ingress_subject("planner", "message.send");

    for method in [A2aMethod::MessageSend, A2aMethod::TasksGet, A2aMethod::TasksList] {
        let decision = gate.evaluate(&ctx(
            method,
            "planner",
            Some("user/alice"),
            &subject,
        ));
        assert!(
            matches!(decision, Tier1DeclarativeDecision::Allow { .. }),
            "expected allow, got {decision:?}"
        );
    }
}

#[test]
fn per_method_allowlist_denies_other_methods() {
    let gate = RealTier1DeclarativeGate::new(load_reference_bundle("per-method-allowlist.tier1.toml"));
    let decision = gate.evaluate(&ctx(
        A2aMethod::TasksCancel,
        "planner",
        Some("user/alice"),
        &ingress_subject("planner", "tasks.cancel"),
    ));
    assert_eq!(
        decision,
        Tier1DeclarativeDecision::Deny {
            rule: Tier1DeclarativeRuleId::new("deny-other-methods")
        }
    );
}

#[test]
fn per_method_allowlist_deny_emits_policy_denied_response_shape() {
    let gate = RealTier1DeclarativeGate::new(load_reference_bundle("per-method-allowlist.tier1.toml"));
    let payload = br#"{"jsonrpc":"2.0","id":"1","method":"tasks/cancel","params":{}}"#;
    let decision = gate.evaluate(&ctx(
        A2aMethod::TasksCancel,
        "planner",
        Some("user/alice"),
        &ingress_subject("planner", "tasks.cancel"),
    ));
    assert!(matches!(decision, Tier1DeclarativeDecision::Deny { .. }));

    let denied_body =
        ingress_gateway_policy_denied_response_bytes(payload, "tier-1 declarative policy rejected envelope")
            .expect("deny response");
    let denied_json: serde_json::Value = serde_json::from_slice(&denied_body).expect("deny json");
    assert_eq!(denied_json["error"]["code"], -32_801);
    assert_eq!(
        tier1_declarative_audit_rule_fired(&decision),
        "gateway.tier1.declarative.denied.deny-other-methods"
    );

    // Full ingress dispatch with declarative deny is covered in
    // `a2a-gateway/src/runtime.rs` (`gateway_dispatch_tests::tier1_declarative_deny_rule_returns_policy_denied_code`).
}

#[test]
fn per_agent_allowlist_reference_bundle_loads() {
    let bundle = load_reference_bundle("per-agent-allowlist.tier1.toml");
    assert_eq!(bundle.rules().len(), 1);
}

#[test]
fn per_agent_allowlist_allows_configured_callers() {
    let gate = RealTier1DeclarativeGate::new(load_reference_bundle("per-agent-allowlist.tier1.toml"));
    let subject = ingress_subject("planner", "message.send");

    for caller in ["user/alice", "user/bob", "service/internal-bot"] {
        let decision = gate.evaluate(&ctx(
            A2aMethod::MessageSend,
            "planner",
            Some(caller),
            &subject,
        ));
        assert_eq!(decision, Tier1DeclarativeDecision::Allow { rule: None });
    }
}

#[test]
fn per_agent_allowlist_denies_other_callers() {
    let gate = RealTier1DeclarativeGate::new(load_reference_bundle("per-agent-allowlist.tier1.toml"));
    let decision = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        Some("user/charlie"),
        &ingress_subject("planner", "message.send"),
    ));
    assert_eq!(
        decision,
        Tier1DeclarativeDecision::Deny {
            rule: Tier1DeclarativeRuleId::new("deny-non-allowlisted-callers")
        }
    );
}

#[test]
fn per_agent_allowlist_denies_anonymous_callers() {
    let gate = RealTier1DeclarativeGate::new(load_reference_bundle("per-agent-allowlist.tier1.toml"));
    let decision = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        None,
        &ingress_subject("planner", "message.send"),
    ));
    assert_eq!(
        decision,
        Tier1DeclarativeDecision::Deny {
            rule: Tier1DeclarativeRuleId::new("deny-non-allowlisted-callers")
        }
    );
}

#[test]
fn time_of_day_reference_bundle_loads() {
    let bundle = load_reference_bundle("time-of-day.tier1.toml");
    assert_eq!(bundle.rules().len(), 1);
}

fn utc_instant(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> std::time::SystemTime {
    let datetime = time::Date::from_calendar_date(year, time::Month::try_from(month).unwrap(), day)
        .unwrap()
        .with_hms(hour, minute, 0)
        .unwrap()
        .assume_utc();
    UNIX_EPOCH + Duration::from_secs(datetime.unix_timestamp() as u64)
}

#[test]
fn time_of_day_after_hours_profile_denies_outside_window() {
    let bundle = load_reference_bundle("time-of-day.tier1.toml");
    let after_hours = utc_instant(2026, 5, 25, 20, 0);
    let gate = RealTier1DeclarativeGate::with_clock(bundle, Arc::new(FixedTier1Clock::new(after_hours)));
    let decision = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        Some("user/alice"),
        &ingress_subject("planner", "message.send"),
    ));
    assert_eq!(
        decision,
        Tier1DeclarativeDecision::Deny {
            rule: Tier1DeclarativeRuleId::new("deny-outside-business-hours")
        }
    );
}

#[test]
fn time_of_day_inside_business_hours_defaults_allow() {
    let bundle = load_reference_bundle("time-of-day.tier1.toml");
    let open_hours = utc_instant(2026, 5, 25, 10, 0);
    let gate = RealTier1DeclarativeGate::with_clock(bundle, Arc::new(FixedTier1Clock::new(open_hours)));
    let decision = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        Some("user/alice"),
        &ingress_subject("planner", "message.send"),
    ));
    assert_eq!(decision, Tier1DeclarativeDecision::Allow { rule: None });
}

#[test]
fn reference_policy_files_exist() {
    let dir = reference_policies_dir();
    for name in [
        "per-method-allowlist.tier1.toml",
        "per-agent-allowlist.tier1.toml",
        "time-of-day.tier1.toml",
        "README.md",
    ] {
        assert!(Path::new(&dir.join(name)).is_file(), "missing {name}");
    }
}
