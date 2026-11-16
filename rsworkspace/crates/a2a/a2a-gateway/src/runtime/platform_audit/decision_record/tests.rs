use std::time::Duration;

use a2a_nats::agent_id::A2aAgentId;
use buffa::EnumValue;
use trogon_audit::{Obligation, ReadOnly, Stamper};
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{
    AuditClass, AuditDecision, EnforcementMode, GatewayDecision, GatewaySurface,
};

use super::*;
use crate::runtime::platform_audit::deny_reason::DenyReason;

const TRACE: &str = "0190a7b2-3c4d-7e5f-8a6b-7c8d9e0f1a2b";

fn agent() -> A2aAgentId {
    A2aAgentId::new("planner").expect("agent")
}

fn person(id: &str) -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", id).expect("principal")
}

fn event<'a>(
    caller: &'a SubjectPrincipal,
    agent_id: &'a A2aAgentId,
    method: &'a str,
    read_only: ReadOnly,
    rules: &'a [String],
    verdict: Verdict<'a>,
) -> DecisionEvent<'a> {
    DecisionEvent {
        caller,
        agent_id,
        method,
        read_only,
        trace_id: TRACE,
        request_id: Some("req-1"),
        request_bytes: 321,
        latency: Duration::from_millis(7),
        rules_fired: rules,
        verdict,
    }
}

fn build(event: &DecisionEvent<'_>) -> Record {
    let stamper = Stamper::new(SystemClock, UuidV7Generator);
    decision_record(
        event,
        &AuditName::new("acme").expect("tenant"),
        &NatsToken::new("payments").expect("project"),
        stamper.stamp(),
    )
    .expect("record")
}

fn gateway(record: &Record) -> &GatewayDecision {
    match record.message().detail.as_ref().expect("detail") {
        Detail::GatewayDecision(decision) => decision,
        other => panic!("not a decision: {other:?}"),
    }
}

fn allowed<'a>(token: Option<&'a str>) -> Verdict<'a> {
    Verdict::Allowed(AllowedLayers {
        tier1_spicedb: true,
        tier1_consistency_token: token,
        tier3: true,
    })
}

#[test]
fn an_allow_of_a_method_that_acts_is_an_enforced_must_audit_allow() {
    let caller = person("alice");
    let agent = agent();
    let rules = vec!["gateway.tier1.spicedb_allowed".to_owned()];
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &rules,
        allowed(None),
    ));
    let message = record.message();
    assert_eq!(record.obligation(), Obligation::BeforeEffect);
    assert_eq!(message.class, EnumValue::from(AuditClass::Must));
    assert_eq!(message.tenant.value, "acme");
    assert_eq!(message.project.value, "payments");
    assert_eq!(
        message.attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_ALLOW)
    );
    assert_eq!(
        message.attribution.enforcement_mode,
        EnumValue::from(EnforcementMode::ENFORCEMENT_MODE_ENFORCE)
    );
    assert_eq!(message.attribution.reason_code, None);
    assert_eq!(
        message.attribution.rule_ids,
        vec!["gateway.tier1.spicedb_allowed".to_owned()]
    );

    let decision = gateway(&record);
    assert_eq!(decision.target, "message/send@planner");
    assert_eq!(
        decision.surface,
        EnumValue::from(GatewaySurface::GATEWAY_SURFACE_A2A_INGRESS)
    );
    assert_eq!(decision.request_bytes, Some(321));
}

#[test]
fn an_allow_of_a_method_declared_read_only_is_best_effort() {
    let caller = person("alice");
    let agent = agent();
    let record = build(&event(
        &caller,
        &agent,
        "tasks/get",
        ReadOnly::Declared,
        &[],
        allowed(None),
    ));
    assert_eq!(record.obligation(), Obligation::BestEffort);
    assert_eq!(record.message().class, EnumValue::from(AuditClass::BestEffort));
}

#[test]
fn a_deny_stands_whether_or_not_the_method_is_read_only() {
    let caller = person("alice");
    let agent = agent();
    for read_only in [ReadOnly::Declared, ReadOnly::Undeclared] {
        let record = build(&event(
            &caller,
            &agent,
            "tasks/get",
            read_only,
            &[],
            Verdict::Denied(DenyReason::Tier2),
        ));
        assert_eq!(record.obligation(), Obligation::Stands, "{read_only:?}");
        assert_eq!(
            record.message().class,
            EnumValue::from(AuditClass::Must),
            "{read_only:?}"
        );
    }
}

#[test]
fn a_deny_carries_its_reason_code_and_the_layer_that_refused() {
    let caller = person("alice");
    let agent = agent();
    let rules = vec!["gateway.tier2.cap".to_owned()];
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &rules,
        Verdict::Denied(DenyReason::Tier2),
    ));
    let attribution = &record.message().attribution;
    assert_eq!(
        attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_DENY)
    );
    assert_eq!(attribution.reason_code.as_deref(), Some("tier2_denied"));
    let layers = &gateway(&record).layer_verdicts;
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].layer, "tier2");
    assert_eq!(layers[0].reason_code.as_deref(), Some("tier2_denied"));
}

#[test]
fn an_allow_names_the_layers_that_evaluated_it_and_the_token_tier_one_decided_at() {
    let caller = person("alice");
    let agent = agent();
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        allowed(Some("GhUKEzE2MzI1")),
    ));
    let layers = &gateway(&record).layer_verdicts;
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].layer, "tier1_spicedb");
    assert_eq!(layers[0].consistency_token.as_deref(), Some("GhUKEzE2MzI1"));
    assert_eq!(layers[1].layer, "tier3");
}

#[test]
fn a_layer_that_was_off_is_not_reported_as_having_allowed() {
    let caller = person("alice");
    let agent = agent();
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        Verdict::Allowed(AllowedLayers::default()),
    ));
    assert!(gateway(&record).layer_verdicts.is_empty());
}

#[test]
fn a_token_a_record_cannot_carry_is_left_off_and_the_allow_still_records() {
    let caller = person("alice");
    let agent = agent();
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        allowed(Some("")),
    ));
    assert_eq!(gateway(&record).layer_verdicts[0].consistency_token, None);
}

#[test]
fn the_gateways_trace_id_and_the_request_id_correlate_the_record() {
    let caller = person("alice");
    let agent = agent();
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        allowed(None),
    ));
    assert_eq!(
        record.message().trace_id.as_deref(),
        Some("0190a7b23c4d7e5f8a6b7c8d9e0f1a2b")
    );
    assert_eq!(record.message().request_id.as_deref(), Some("req-1"));
}

#[test]
fn a_trace_id_or_request_id_a_record_cannot_carry_is_left_off() {
    let caller = person("alice");
    let agent = agent();
    let mut event = event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        allowed(None),
    );
    event.trace_id = "not-a-uuid";
    event.request_id = Some("has space");
    let record = build(&event);
    assert_eq!(record.message().trace_id, None);
    assert_eq!(record.message().request_id, None);
}

#[test]
fn a_person_is_a_human_session_issued_by_the_tenant() {
    let caller = person("alice");
    let agent = agent();
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        allowed(None),
    ));
    let actor = format!("{:?}", record.message().actor);
    assert!(actor.contains("alice"), "{actor}");
    assert!(actor.contains("acme"), "{actor}");
}

#[test]
fn an_agent_is_an_agent_actor() {
    let caller = SubjectPrincipal::from_tokens("agent", "worker-1").expect("principal");
    let agent = agent();
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        allowed(None),
    ));
    let actor = format!("{:?}", record.message().actor);
    assert!(actor.contains("worker-1"), "{actor}");
    assert!(actor.to_lowercase().contains("agent"), "{actor}");
}

#[test]
fn a_service_has_no_actor_arm_and_is_unattributed() {
    let caller = SubjectPrincipal::from_tokens("service", "billing").expect("principal");
    let agent = agent();
    let record = build(&event(
        &caller,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        allowed(None),
    ));
    let actor = format!("{:?}", record.message().actor);
    assert!(!actor.contains("billing"), "{actor}");
    assert!(actor.to_lowercase().contains("unattributed"), "{actor}");
}

#[test]
fn a_hashed_principal_id_is_recorded_under_the_token_the_bus_carries() {
    let token = trogon_identity_types::SubjectPrincipalId::from_credential(
        &trogon_identity_types::PrincipalId::new("alice@example.com").expect("id"),
    )
    .expect("subject id");
    let hashed = SubjectPrincipal::new(trogon_identity_types::PrincipalKind::Person, token.clone());
    let agent = agent();
    let record = build(&event(
        &hashed,
        &agent,
        "message/send",
        ReadOnly::Undeclared,
        &[],
        allowed(None),
    ));
    let actor = format!("{:?}", record.message().actor);
    assert!(actor.contains(token.as_str()), "{actor}");
    assert!(!actor.contains("example.com"), "{actor}");
}
