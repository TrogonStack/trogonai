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

fn make_subject(prefix: &str, outcome: IngressAuditOutcome, skill: &SkillId) -> String {
    ingress_audit_subject(prefix, outcome, skill).expect("valid test audit subject")
}

#[test]
fn subject_segments_skill_with_dots() {
    // `.` escapes to `_d` so the skill segment is one NATS token.
    let subject = make_subject("a2a", IngressAuditOutcome::Allow, &skill("docs.search"));
    assert_eq!(subject, "a2a.a2a.audit.allow.ingress.docs_dsearch");
}

#[test]
fn subject_replaces_each_nats_unsafe_char() {
    // `.`, `*`, `>` each split or wildcard a subject; reversible
    // escapes `_d`/`_s`/`_g` keep the segment a single token.
    let subject = make_subject("a2a", IngressAuditOutcome::Deny, &skill("a.b*c>d"));
    assert_eq!(subject, "a2a.a2a.audit.deny.ingress.a_db_sc_gd");
}

#[test]
fn subject_encodes_ascii_space_as_hex_codepoint() {
    // Whitespace escapes to `_u<hex>` (per-codepoint) so distinct
    // whitespace code points (ordinary space, no-break space, …)
    // can't alias on the same subject. ASCII space is the only
    // whitespace SkillId actually allows, but the encoding is
    // identical to any other codepoint that lands here.
    let subject = make_subject("a2a", IngressAuditOutcome::Allow, &skill("a b c"));
    assert_eq!(subject, "a2a.a2a.audit.allow.ingress.a_u000020b_u000020c");
}

#[test]
fn skill_token_distinguishes_unicode_whitespace_codepoints() {
    // Ordinary space (U+0020) and no-break space (U+00A0) are both
    // `is_whitespace()` true and both reachable via SkillId, so a
    // shared `_w` escape would alias them. Per-codepoint hex keeps
    // them distinguishable on the audit subject.
    let space = make_subject("a2a", IngressAuditOutcome::Allow, &skill("a b"));
    let nbsp = make_subject("a2a", IngressAuditOutcome::Allow, &skill("a\u{00a0}b"));
    assert_ne!(
        space, nbsp,
        "different whitespace codepoints must produce different subjects"
    );
    assert!(space.contains("_u000020"));
    assert!(nbsp.contains("_u0000a0"));
}

#[test]
fn skill_token_avoids_underscore_collision() {
    // Distinct skill ids that differ only in `.` vs `_` used to
    // collide on the same audit subject because both mapped to
    // `_`. The reversible escape (`_` → `__`, `.` → `_d`) keeps
    // them distinguishable.
    let dotted = make_subject("a2a", IngressAuditOutcome::Allow, &skill("docs.search"));
    let underscored = make_subject("a2a", IngressAuditOutcome::Allow, &skill("docs_search"));
    assert_ne!(
        dotted, underscored,
        "`docs.search` and `docs_search` must publish to different subjects",
    );
    assert_eq!(underscored, "a2a.a2a.audit.allow.ingress.docs__search");
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
fn value_object_accessors_round_trip_through_as_str() {
    // `.as_str()` is the post-construction read path callers use to
    // emit the validated value into logs, metrics, or downstream
    // wire payloads. Cover every newtype so a future refactor that
    // re-wraps the inner String can't silently truncate the view.
    let r = AuditRequestId::new("req-7").expect("valid");
    assert_eq!(r.as_str(), "req-7");
    let t = AuditTenant::new("acme").expect("valid");
    assert_eq!(t.as_str(), "acme");
    let a = AuditAgent::new("planner").expect("valid");
    assert_eq!(a.as_str(), "planner");
    let tp = AuditTraceparent::new("00-aaa-bbb-01").expect("valid");
    assert_eq!(tp.as_str(), "00-aaa-bbb-01");
}

#[test]
fn skill_token_escapes_non_ascii() {
    // SkillId allows non-ASCII text (e.g. emoji, accented chars),
    // but NatsToken is ASCII-only. The helper escapes each non-
    // ASCII char to `_u<hex>` so the produced subject remains
    // publishable.
    let subject = make_subject("a2a", IngressAuditOutcome::Allow, &skill("café"));
    assert_eq!(subject, "a2a.a2a.audit.allow.ingress.caf_u0000e9");
}

#[test]
fn ingress_audit_subject_rejects_overlong_segment() {
    // After escaping, the skill segment can exceed the NATS
    // per-token length cap (128 chars). We surface that via
    // `InvalidSkillSegment` rather than emitting a subject the
    // publisher would reject.
    let long = "a".repeat(200);
    let err = ingress_audit_subject("a2a", IngressAuditOutcome::Allow, &skill(&long)).expect_err("overlong must error");
    assert!(matches!(
        err,
        IngressAuditSubjectError::InvalidSkillSegment(SubjectTokenViolation::TooLong(_))
    ));
}

#[test]
fn deserializing_empty_value_object_strings_rejects_them() {
    // `#[serde(try_from = "String")]` routes deserialization through
    // `new`, so wire-borne empty/whitespace values fail-closed the
    // same way as construction would. Without this, a transparent
    // deserialize would silently smuggle invalid correlation keys
    // into the envelope.
    let cases = [
        (r#""""#, "AuditRequestId"),
        (r#""   ""#, "AuditTenant"),
        (r#""\t""#, "AuditAgent"),
    ];
    assert!(serde_json::from_str::<AuditRequestId>(cases[0].0).is_err());
    assert!(serde_json::from_str::<AuditTenant>(cases[1].0).is_err());
    assert!(serde_json::from_str::<AuditAgent>(cases[2].0).is_err());
    assert!(serde_json::from_str::<AuditTraceparent>(r#""""#).is_err());
}

#[test]
fn deserializing_valid_value_objects_round_trips() {
    let r: AuditRequestId = serde_json::from_str("\"req-9\"").expect("valid");
    assert_eq!(r.as_str(), "req-9");
    let t: AuditTenant = serde_json::from_str("\"acme\"").expect("valid");
    assert_eq!(t.as_str(), "acme");
    let a: AuditAgent = serde_json::from_str("\"planner\"").expect("valid");
    assert_eq!(a.as_str(), "planner");
    let tp: AuditTraceparent = serde_json::from_str("\"00-aaa-bbb-01\"").expect("valid");
    assert_eq!(tp.as_str(), "00-aaa-bbb-01");
}

#[test]
fn resolved_rule_round_trip_shape() {
    // Compile-only check that ResolvedRule is reachable as part of
    // the public surface — audit consumers introspect on this for
    // post-hoc analysis.
    let _ = std::mem::size_of::<ResolvedRule>();
}
