use super::*;
use crate::policy::per_skill::ResolvedRule;

fn skill(s: &str) -> SkillId {
    SkillId::new(s).expect("non-empty test skill id")
}

fn agent(s: &str) -> AuditAgent {
    AuditAgent::new(s).expect("non-empty test agent")
}

fn rid(s: &str) -> AuditRequestId {
    AuditRequestId::new(s).expect("non-empty test request id")
}

fn tenant(s: &str) -> AuditTenant {
    AuditTenant::new(s).expect("non-empty test tenant")
}

#[test]
fn subject_segments_skill_with_dots() {
    let subject = ingress_audit_subject("a2a", IngressAuditOutcome::Allow, &skill("docs.search"));
    assert_eq!(subject, "a2a.a2a.audit.allow.ingress.docs_search");
}

#[test]
fn subject_replaces_each_nats_unsafe_char() {
    // `.`, `*`, `>` each split or wildcard a subject. Replacing them
    // keeps the skill segment a single token.
    let subject = ingress_audit_subject("a2a", IngressAuditOutcome::Deny, &skill("a.b*c>d"));
    assert_eq!(subject, "a2a.a2a.audit.deny.ingress.a_b_c_d");
}

#[test]
fn subject_normalizes_plain_ascii_space() {
    // ASCII space is the only whitespace character that survives
    // `SkillId::new` (which rejects control chars). The helper still
    // normalizes the broader `is_whitespace` class as
    // defense-in-depth, but the path actually exercised in
    // production goes through this case.
    let subject = ingress_audit_subject("a2a", IngressAuditOutcome::Allow, &skill("a b c"));
    assert_eq!(subject, "a2a.a2a.audit.allow.ingress.a_b_c");
}

#[test]
fn audit_envelope_from_allow_decision() {
    let dec = PerSkillDecision::Allow { contributor: None };
    let audit = A2aIngressAudit::from_decision(
        rid("req-1"),
        tenant("acme"),
        agent("planner"),
        skill("search"),
        &dec,
        Some(AuditTraceparent::new("00-aaa-bbb-01").expect("non-empty")),
    );
    assert_eq!(audit.outcome, IngressAuditOutcome::Allow);
    assert_eq!(audit.reason, None);
    assert!(!audit.shadow);
    assert_eq!(
        audit.traceparent.as_ref().map(AuditTraceparent::as_str),
        Some("00-aaa-bbb-01")
    );
}

#[test]
fn audit_envelope_from_deny_decision() {
    let dec = PerSkillDecision::Deny {
        reason: "blocked".into(),
        contributor: None,
    };
    let audit = A2aIngressAudit::from_decision(
        rid("req-1"),
        tenant("acme"),
        agent("planner"),
        skill("search"),
        &dec,
        None,
    );
    assert_eq!(audit.outcome, IngressAuditOutcome::Deny);
    assert_eq!(audit.reason.as_deref(), Some("blocked"));
    assert!(!audit.shadow);
}

#[test]
fn audit_envelope_marks_shadow_with_deny_inner() {
    let dec = PerSkillDecision::Shadow {
        would_be: Box::new(PerSkillDecision::Deny {
            reason: "forbidden".into(),
            contributor: None,
        }),
    };
    let audit = A2aIngressAudit::from_decision(
        rid("req-2"),
        tenant("acme"),
        agent("planner"),
        skill("search"),
        &dec,
        None,
    );
    assert_eq!(audit.outcome, IngressAuditOutcome::Deny);
    assert_eq!(audit.reason.as_deref(), Some("forbidden"));
    assert!(audit.shadow);
}

#[test]
fn audit_envelope_marks_shadow_with_allow_inner() {
    let dec = PerSkillDecision::Shadow {
        would_be: Box::new(PerSkillDecision::Allow { contributor: None }),
    };
    let audit = A2aIngressAudit::from_decision(
        rid("req-3"),
        tenant("acme"),
        agent("planner"),
        skill("search"),
        &dec,
        None,
    );
    assert_eq!(audit.outcome, IngressAuditOutcome::Allow);
    assert!(audit.shadow);
}

#[test]
fn audit_envelope_unwraps_nested_shadow() {
    // Constructing nested Shadow isn't the intended usage but the
    // type allows it — classify must still report the terminal
    // outcome rather than a synthetic "unknown" so the audit
    // consumer doesn't see a tag it can't route.
    let dec = PerSkillDecision::Shadow {
        would_be: Box::new(PerSkillDecision::Shadow {
            would_be: Box::new(PerSkillDecision::Deny {
                reason: "deep".into(),
                contributor: None,
            }),
        }),
    };
    let audit = A2aIngressAudit::from_decision(
        rid("req-4"),
        tenant("acme"),
        agent("planner"),
        skill("search"),
        &dec,
        None,
    );
    assert_eq!(audit.outcome, IngressAuditOutcome::Deny);
    assert_eq!(audit.reason.as_deref(), Some("deep"));
    assert!(audit.shadow);
}

#[test]
fn ingress_audit_outcome_serializes_snake_case() {
    // The serialized form lands in audit-stream payloads — pin it so
    // a future enum rename doesn't quietly invalidate downstream
    // consumers' filter rules.
    assert_eq!(
        serde_json::to_string(&IngressAuditOutcome::Allow).expect("serialize"),
        "\"allow\""
    );
    assert_eq!(
        serde_json::to_string(&IngressAuditOutcome::Deny).expect("serialize"),
        "\"deny\""
    );
}

#[test]
fn ingress_audit_outcome_as_str_matches_serialized_form() {
    assert_eq!(IngressAuditOutcome::Allow.as_str(), "allow");
    assert_eq!(IngressAuditOutcome::Deny.as_str(), "deny");
}

#[test]
fn value_object_constructors_reject_empty_and_whitespace() {
    // Each constructor must fail-closed on empty/whitespace input so
    // an envelope can't be built with a correlation key that aliases
    // every untraced request to the same row.
    assert_eq!(
        AuditRequestId::new("").unwrap_err(),
        A2aIngressAuditBuildError::EmptyRequestId
    );
    assert_eq!(
        AuditRequestId::new("   ").unwrap_err(),
        A2aIngressAuditBuildError::EmptyRequestId
    );
    assert_eq!(
        AuditTenant::new("").unwrap_err(),
        A2aIngressAuditBuildError::EmptyTenant
    );
    assert_eq!(AuditAgent::new("").unwrap_err(), A2aIngressAuditBuildError::EmptyAgent);
    assert_eq!(
        AuditTraceparent::new("").unwrap_err(),
        A2aIngressAuditBuildError::EmptyTraceparent
    );
}

#[test]
fn audit_envelope_serialization_preserves_value_object_shapes() {
    // The serde representation is the downstream wire contract. The
    // `#[serde(transparent)]` newtypes serialize as bare strings, so
    // a renamed inner field can't silently break consumers.
    let audit = A2aIngressAudit::from_decision(
        rid("req-5"),
        tenant("acme"),
        agent("planner"),
        skill("search"),
        &PerSkillDecision::Allow { contributor: None },
        None,
    );
    let json = serde_json::to_value(&audit).expect("serialize");
    assert_eq!(json["request_id"], "req-5");
    assert_eq!(json["tenant"], "acme");
    assert_eq!(json["agent"], "planner");
    assert_eq!(json["outcome"], "allow");
    assert_eq!(json["shadow"], false);
}

#[test]
fn resolved_rule_round_trip_shape() {
    // Compile-only check that ResolvedRule is reachable as part of
    // the public surface — audit consumers introspect on this for
    // post-hoc analysis.
    let _ = std::mem::size_of::<ResolvedRule>();
}
