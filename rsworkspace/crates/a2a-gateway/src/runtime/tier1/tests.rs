use a2a_nats::audit::envelope::AuditEnvelopeFields;

use super::*;

fn agent(s: &str) -> A2aAgentId {
    A2aAgentId::new(s).expect("nats-safe test agent id")
}

#[test]
fn declarative_context_returns_none_for_unknown_method_dots() {
    // Unknown method dots must surface as `None` so the dispatch
    // path routes the request to the invalid-method error instead
    // of running policy with a synthesized method.
    let ctx = tier1_declarative_context_from_ingress(
        "does.not.exist",
        &agent("planner"),
        Some("user/alice"),
        "acme",
        "a2a.gateway.planner.message.send",
    );
    assert!(ctx.is_none());
}

#[test]
fn declarative_context_resolves_slash_caller_slug() {
    // `user/alice` is the canonical principal-claim form; the
    // helper must strip the `user/` type prefix so the bundle
    // matches against the bare id.
    let ctx = tier1_declarative_context_from_ingress(
        "message.send",
        &agent("planner"),
        Some("user/alice"),
        "acme",
        "a2a.gateway.planner.message.send",
    )
    .expect("context for message.send");
    // The caller_subject ends up wrapping `user/alice` (the slug
    // is stripped on the way into the principal, then the
    // principal's spicedb_subject reconstructs `user/<slug>`).
    let subject = ctx.caller_subject.as_ref().expect("subject present");
    assert!(subject.as_str().contains("alice"));
}

#[test]
fn declarative_context_normalizes_plain_slug() {
    // A caller value without the `user/` prefix should still
    // produce a usable subject -- this is the shape that comes
    // from JWT subjects that already carry just the id.
    let ctx = tier1_declarative_context_from_ingress(
        "message.send",
        &agent("planner"),
        Some("alice"),
        "acme",
        "a2a.gateway.planner.message.send",
    )
    .expect("context");
    let subject = ctx.caller_subject.as_ref().expect("subject present");
    assert!(subject.as_str().contains("alice"));
}

#[test]
fn declarative_context_uses_anonymous_sentinel_when_caller_absent() {
    // Absent caller resolves to the anonymous placeholder so the
    // bundle can match deny-anonymous rules explicitly.
    let ctx = tier1_declarative_context_from_ingress(
        "message.send",
        &agent("planner"),
        None,
        "acme",
        "a2a.gateway.planner.message.send",
    )
    .expect("context");
    let subject = ctx.caller_subject.as_ref().expect("subject present");
    assert!(subject.as_str().contains(ANONYMOUS_CALLER_SLUG));
}

#[test]
fn anonymous_caller_slug_is_underscore() {
    // Pin the literal so bundle authors writing deny-anonymous
    // rules against `user/_` don't drift away from this constant.
    assert_eq!(ANONYMOUS_CALLER_SLUG, "_");
}

#[test]
fn declarative_context_round_trips_method_and_agent() {
    let ctx = tier1_declarative_context_from_ingress(
        "tasks.get",
        &agent("worker"),
        Some("user/bob"),
        "acme",
        "a2a.gateway.worker.tasks.get",
    )
    .expect("context");
    assert_eq!(ctx.agent_method, A2aMethod::TasksGet);
    assert_eq!(ctx.agent_id.as_str(), "worker");
    assert_eq!(ctx.nats_subject, "a2a.gateway.worker.tasks.get");
}

#[test]
fn enrich_audit_caller_stamps_both_fields() {
    let fields = enrich_audit_caller(
        AuditEnvelopeFields::default(),
        "user/alice",
        &Some("jwt-mint".to_owned()),
    );
    assert_eq!(fields.caller_id.as_deref(), Some("user/alice"));
    assert_eq!(fields.caller_source.as_deref(), Some("jwt-mint"));
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
    let enriched = enrich_audit_caller(fields, "user/alice", &None);
    assert_eq!(enriched.trace_id.as_deref(), Some("trace-1"));
    assert!(
        enriched
            .rules_fired
            .as_ref()
            .expect("rules")
            .contains(&"gateway.tier1.spicedb_denied".to_owned())
    );
    assert_eq!(enriched.caller_id.as_deref(), Some("user/alice"));
    assert!(enriched.caller_source.is_none());
}

#[test]
fn enrich_audit_caller_overwrites_caller_pair_only() {
    // If `caller_id` was somehow pre-populated, the enricher
    // should win (most recent identity).  This is the path used
    // when the denial flow re-stamps the caller pair from a
    // later, more authoritative source.
    let fields = AuditEnvelopeFields {
        caller_id: Some("user/old".into()),
        caller_source: Some("legacy".into()),
        ..Default::default()
    };
    let enriched = enrich_audit_caller(fields, "user/new", &Some("jwt".into()));
    assert_eq!(enriched.caller_id.as_deref(), Some("user/new"));
    assert_eq!(enriched.caller_source.as_deref(), Some("jwt"));
}
