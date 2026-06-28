use super::*;
use crate::policy::per_skill::ResolvedRule;

fn skill(s: &str) -> SkillId {
    SkillId::new(s).expect("non-empty test skill id")
}

#[test]
fn subject_segments_skill_with_dots() {
    let subject = ingress_audit_subject("a2a", IngressAuditOutcome::Allow, &skill("docs.search"));
    assert_eq!(subject, "a2a.a2a.audit.allow.ingress.docs_search");
}

#[test]
fn subject_replaces_each_nats_unsafe_char() {
    // `.`, ` `, `*`, `>` each split or wildcard a subject — replacing
    // them keeps the skill segment a single token. Cover the full set
    // so adding/removing one without updating this test surfaces.
    let subject = ingress_audit_subject("a2a", IngressAuditOutcome::Deny, &skill("a.b c*d>e"));
    assert_eq!(subject, "a2a.a2a.audit.deny.ingress.a_b_c_d_e");
}

#[test]
fn audit_envelope_from_allow_decision() {
    let dec = PerSkillDecision::Allow { contributor: None };
    let audit = A2aIngressAudit::from_decision(
        "req-1",
        "acme",
        "planner",
        skill("search"),
        &dec,
        Some("00-aaa-bbb-01".into()),
    );
    assert_eq!(audit.outcome, IngressAuditOutcome::Allow);
    assert_eq!(audit.reason, None);
    assert!(!audit.shadow);
    assert_eq!(audit.traceparent.as_deref(), Some("00-aaa-bbb-01"));
}

#[test]
fn audit_envelope_from_deny_decision() {
    let dec = PerSkillDecision::Deny {
        reason: "blocked".into(),
        contributor: None,
    };
    let audit = A2aIngressAudit::from_decision("req-1", "acme", "planner", skill("search"), &dec, None);
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
    let audit = A2aIngressAudit::from_decision("req-2", "acme", "planner", skill("search"), &dec, None);
    assert_eq!(audit.outcome, IngressAuditOutcome::Deny);
    assert_eq!(audit.reason.as_deref(), Some("forbidden"));
    assert!(audit.shadow);
}

#[test]
fn audit_envelope_marks_shadow_with_allow_inner() {
    let dec = PerSkillDecision::Shadow {
        would_be: Box::new(PerSkillDecision::Allow { contributor: None }),
    };
    let audit = A2aIngressAudit::from_decision("req-3", "acme", "planner", skill("search"), &dec, None);
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
    let audit = A2aIngressAudit::from_decision("req-4", "acme", "planner", skill("search"), &dec, None);
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
fn resolved_rule_round_trip_shape() {
    // Compile-only check that ResolvedRule is reachable as part of
    // the public surface — audit consumers introspect on this for
    // post-hoc analysis.
    let _ = std::mem::size_of::<ResolvedRule>();
}
