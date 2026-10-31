use a2a_nats::audit::envelope::AuditEnvelopeFields;

use super::*;

fn agent(s: &str) -> A2aAgentId {
    A2aAgentId::new(s).expect("nats-safe test agent id")
}

fn principal(kind: &str, id: &str) -> SubjectPrincipal {
    SubjectPrincipal::from_tokens(kind, id).expect("principal")
}

fn caller(kind: &str, id: &str) -> CallerSpiceDbSubject {
    CallerSpiceDbSubject::from(&principal(kind, id))
}

#[test]
fn declarative_context_returns_none_for_unknown_method_dots() {
    // Unknown method dots must surface as `None` so the dispatch
    // path routes the request to the invalid-method error instead
    // of running policy with a synthesized method.
    let ctx = tier1_declarative_context_from_ingress(
        "does.not.exist",
        &agent("planner"),
        &caller("person", "alice"),
        "a2a.v1.gateway.planner.person.alice.message.send",
    );
    assert!(ctx.is_none());
}

#[test]
fn declarative_context_carries_the_subject_principal_as_its_spicedb_subject() {
    for (kind, id, expected) in [
        ("person", "alice", "user/alice"),
        ("agent", "scout", "agent/scout"),
        ("service", "internal-cron", "service/internal-cron"),
    ] {
        let ctx = tier1_declarative_context_from_ingress(
            "message.send",
            &agent("planner"),
            &caller(kind, id),
            "a2a.v1.gateway.planner.person.alice.message.send",
        )
        .expect("context for message.send");

        assert_eq!(ctx.caller_subject.as_str(), expected);
    }
}

#[test]
fn declarative_context_round_trips_method_and_agent() {
    let ctx = tier1_declarative_context_from_ingress(
        "tasks.get",
        &agent("worker"),
        &caller("person", "bob"),
        "a2a.v1.gateway.worker.person.bob.tasks.get",
    )
    .expect("context");
    assert_eq!(ctx.agent_method, A2aMethod::TasksGet);
    assert_eq!(ctx.agent_id.as_str(), "worker");
    assert_eq!(ctx.nats_subject, "a2a.v1.gateway.worker.person.bob.tasks.get");
}

#[test]
fn enrich_audit_caller_stamps_the_subject_principal() {
    let fields = enrich_audit_caller(AuditEnvelopeFields::default(), &principal("person", "alice"));
    assert_eq!(fields.caller_id.as_deref(), Some("person.alice"));
}

#[test]
fn enrich_audit_caller_preserves_existing_extras() {
    // Pre-existing extras (e.g. trace_id, rules_fired) must not
    // be cleared just because we're layering caller info on. The
    // dispatch path stamps these in sequence and the audit
    // consumer expects all of them.
    let fields = AuditEnvelopeFields {
        trace_id: Some("trace-1".into()),
        rules_fired: Some(vec!["gateway.tier1.spicedb_denied".into()]),
        ..Default::default()
    };
    let enriched = enrich_audit_caller(fields, &principal("person", "alice"));
    assert_eq!(enriched.trace_id.as_deref(), Some("trace-1"));
    assert!(
        enriched
            .rules_fired
            .as_ref()
            .expect("rules")
            .contains(&"gateway.tier1.spicedb_denied".to_owned())
    );
    assert_eq!(enriched.caller_id.as_deref(), Some("person.alice"));
}

#[test]
fn enrich_audit_caller_overwrites_a_prior_caller() {
    let fields = AuditEnvelopeFields {
        caller_id: Some("person.old".into()),
        ..Default::default()
    };
    let enriched = enrich_audit_caller(fields, &principal("person", "new"));
    assert_eq!(enriched.caller_id.as_deref(), Some("person.new"));
}
