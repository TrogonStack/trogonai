use a2a_auth_callout::SpiceDbSubject;

use super::*;
use crate::policy::tier1_declarative::bundle::{
    Tier1DeclarativeEffect, Tier1DeclarativeMatch, Tier1DeclarativeRule, Tier1DeclarativeRuleId, Tier1ResourceKind,
};

fn ctx(method: A2aMethod, agent: &str, caller: Option<&str>, subject: &str) -> Tier1DeclarativeContext {
    Tier1DeclarativeContext::new(
        method,
        A2aAgentId::new(agent).expect("agent id"),
        caller.map(SpiceDbSubject::new),
        subject,
    )
}

fn rid(id: &str) -> Tier1DeclarativeRuleId {
    Tier1DeclarativeRuleId::new(id).expect("test rule id non-empty")
}

fn m(kind: Tier1ResourceKind, pattern: &str, negate: bool) -> Tier1DeclarativeMatch {
    Tier1DeclarativeMatch::new(kind, pattern, negate).expect("test match pattern valid")
}

fn rule(
    id: &str,
    priority: u32,
    effect: Tier1DeclarativeEffect,
    matches: Vec<Tier1DeclarativeMatch>,
) -> Tier1DeclarativeRule {
    Tier1DeclarativeRule {
        id: rid(id),
        matches,
        effect,
        priority,
    }
}

#[test]
fn matching_rule_returns_effect() {
    let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![rule(
        "deny-planner",
        100,
        Tier1DeclarativeEffect::Deny,
        vec![
            m(Tier1ResourceKind::AgentId, "planner", false),
            m(Tier1ResourceKind::AgentMethod, "message/send", false),
        ],
    )]));

    let decision = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        Some("user/alice"),
        "a2a.gateway.planner.message.send",
    ));
    assert_eq!(
        decision,
        Tier1DeclarativeDecision::Deny {
            rule: rid("deny-planner")
        }
    );
}

#[test]
fn partial_match_tries_next_rule() {
    let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![
        rule(
            "deny-planner",
            100,
            Tier1DeclarativeEffect::Deny,
            vec![m(Tier1ResourceKind::AgentId, "other", false)],
        ),
        rule(
            "allow-planner",
            50,
            Tier1DeclarativeEffect::Allow,
            vec![m(Tier1ResourceKind::AgentId, "planner", false)],
        ),
    ]));

    let decision = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        None,
        "a2a.gateway.planner.message.send",
    ));
    assert_eq!(
        decision,
        Tier1DeclarativeDecision::Allow {
            rule: Some(rid("allow-planner"))
        }
    );
}

#[test]
fn priority_order_is_respected() {
    let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![
        rule(
            "allow-low",
            10,
            Tier1DeclarativeEffect::Allow,
            vec![m(Tier1ResourceKind::AgentId, "planner", false)],
        ),
        rule(
            "deny-high",
            100,
            Tier1DeclarativeEffect::Deny,
            vec![m(Tier1ResourceKind::AgentId, "planner", false)],
        ),
    ]));

    let decision = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        None,
        "a2a.gateway.planner.message.send",
    ));
    assert_eq!(decision, Tier1DeclarativeDecision::Deny { rule: rid("deny-high") });
}

#[test]
fn exact_and_glob_patterns_work() {
    assert!(pattern_matches("message/send", "message/send"));
    assert!(!pattern_matches("message/send", "message/stream"));
    assert!(pattern_matches("user/*", "user/alice"));
    assert!(pattern_matches(
        "a2a.gateway.*.message.send",
        "a2a.gateway.planner.message.send"
    ));
    assert!(!pattern_matches(
        "a2a.gateway.*.message.send",
        "a2a.gateway.planner.message.stream"
    ));
}

#[test]
fn negate_inverts_match() {
    let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![rule(
        "deny-non-alice",
        100,
        Tier1DeclarativeEffect::Deny,
        vec![m(Tier1ResourceKind::CallerSubject, "user/alice", true)],
    )]));

    let denied = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        Some("user/bob"),
        "a2a.gateway.planner.message.send",
    ));
    assert_eq!(
        denied,
        Tier1DeclarativeDecision::Deny {
            rule: rid("deny-non-alice")
        }
    );

    let allowed = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        Some("user/alice"),
        "a2a.gateway.planner.message.send",
    ));
    assert_eq!(allowed, Tier1DeclarativeDecision::Allow { rule: None });
}

#[test]
fn caller_subject_wildcard_does_not_match_unauthenticated_request() {
    // `caller_subject = "*"` must only match when the request actually
    // carries a caller identity. Without this, `None.unwrap_or_default()`
    // becomes `""` and the wildcard pattern would silently allow/deny
    // unauthenticated traffic the rule was meant to scope by caller.
    let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![rule(
        "deny-any-caller",
        100,
        Tier1DeclarativeEffect::Deny,
        vec![m(Tier1ResourceKind::CallerSubject, "*", false)],
    )]));

    let unauthenticated = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        None,
        "a2a.gateway.planner.message.send",
    ));
    assert_eq!(unauthenticated, Tier1DeclarativeDecision::Allow { rule: None });

    let authenticated = gate.evaluate(&ctx(
        A2aMethod::MessageSend,
        "planner",
        Some("user/alice"),
        "a2a.gateway.planner.message.send",
    ));
    assert_eq!(
        authenticated,
        Tier1DeclarativeDecision::Deny {
            rule: rid("deny-any-caller")
        }
    );
}

#[test]
fn noop_gate_defaults_allow() {
    let gate = NoopTier1DeclarativeGate;
    assert!(!gate.is_enabled());
    assert_eq!(
        gate.evaluate(&ctx(
            A2aMethod::MessageSend,
            "planner",
            None,
            "a2a.gateway.planner.message.send",
        )),
        Tier1DeclarativeDecision::Allow { rule: None }
    );
}

#[test]
fn audit_rule_fired_labels_cover_every_decision_variant() {
    assert_eq!(
        tier1_declarative_audit_rule_fired(&Tier1DeclarativeDecision::Allow { rule: None }),
        "gateway.tier1.declarative.no_match_default_allow"
    );
    assert_eq!(
        tier1_declarative_audit_rule_fired(&Tier1DeclarativeDecision::Allow { rule: Some(rid("ok")) }),
        "gateway.tier1.declarative.allowed.ok"
    );
    assert_eq!(
        tier1_declarative_audit_rule_fired(&Tier1DeclarativeDecision::Deny { rule: rid("blocked") }),
        "gateway.tier1.declarative.denied.blocked"
    );
}

#[test]
fn from_env_returns_noop_when_disabled() {
    let env = trogon_std::env::InMemoryEnv::new();
    let layer = Tier1DeclarativeConfig::from_env(&env).expect("noop on missing flag");
    assert!(!layer.gate.is_enabled());
}

#[test]
fn from_env_missing_bundle_dir_when_enabled_is_typed_error() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_TIER1_DECLARATIVE_ENABLED, "on");
    let err = Tier1DeclarativeConfig::from_env(&env).expect_err("missing dir");
    assert!(matches!(err, Tier1DeclarativeBuildError::MissingBundleDir));
}

#[test]
fn from_env_empty_bundle_dir_is_invalid() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_TIER1_DECLARATIVE_ENABLED, "1");
    env.set(ENV_TIER1_BUNDLE_DIR, "   ");
    let err = Tier1DeclarativeConfig::from_env(&env).expect_err("blank dir");
    assert!(matches!(err, Tier1DeclarativeBuildError::BundleDirEmpty));
}

#[test]
fn from_env_unrecognized_enablement_value_is_typed_error() {
    // Operator typos like `=treu` previously disabled the policy
    // silently; the typed error now fails fast at boot.
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_TIER1_DECLARATIVE_ENABLED, "treu");
    let err = Tier1DeclarativeConfig::from_env(&env).expect_err("typo rejected");
    assert!(matches!(
        err,
        Tier1DeclarativeBuildError::EnablementNotBoolean { ref value } if value == "treu"
    ));
}

#[test]
fn from_env_loads_bundle_from_valid_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("allow.tier1.toml"),
        r#"
            [[rule]]
            id = "always-allow"
            priority = 1
            effect = "allow"

            [[rule.matches]]
            kind = "agent_id"
            pattern = "*"
        "#,
    )
    .expect("write bundle");
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_TIER1_DECLARATIVE_ENABLED, "true");
    env.set(ENV_TIER1_BUNDLE_DIR, dir.path().to_str().expect("path utf8"));
    let layer = Tier1DeclarativeConfig::from_env(&env).expect("loads bundle");
    assert!(layer.gate.is_enabled());
}

#[test]
fn rule_id_display_round_trips_through_string() {
    let id = rid("rule-1");
    assert_eq!(format!("{id}"), "rule-1");
    assert_eq!(id.as_str(), "rule-1");
}

#[test]
fn rule_id_construction_rejects_empty_string() {
    let err = Tier1DeclarativeRuleId::new("   ").unwrap_err();
    assert!(matches!(
        err,
        crate::policy::tier1_declarative::Tier1DeclarativeSchemaError::EmptyRuleId
    ));
}

#[test]
fn match_construction_rejects_empty_pattern() {
    let err = Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentId, "   ", false).unwrap_err();
    assert!(matches!(
        err,
        crate::policy::tier1_declarative::Tier1DeclarativeSchemaError::EmptyPattern
    ));
}

#[test]
fn match_construction_validates_time_of_day_pattern() {
    let err = Tier1DeclarativeMatch::new(Tier1ResourceKind::TimeOfDay, "not-a-window", false).unwrap_err();
    assert!(matches!(
        err,
        crate::policy::tier1_declarative::Tier1DeclarativeSchemaError::InvalidTimeOfDayPattern(_)
    ));
}

#[test]
fn fixed_clock_returns_configured_instant() {
    let now = std::time::SystemTime::now();
    let clock = FixedTier1Clock::new(now);
    assert_eq!(clock.now(), now);
}

#[test]
fn nats_star_matches_exactly_one_token() {
    // NATS semantics: `*` matches one dot-separated token only. Without
    // this, a generic glob would let `a2a.gateway.*.message.send` also
    // match `a2a.gateway.tenant.x.message.send` and skew decisions.
    assert!(nats_subject_matches(
        "a2a.gateway.*.message.send",
        "a2a.gateway.planner.message.send"
    ));
    assert!(!nats_subject_matches(
        "a2a.gateway.*.message.send",
        "a2a.gateway.tenant.planner.message.send"
    ));
    assert!(!nats_subject_matches("a2a.gateway.*", "a2a.gateway"));
}

#[test]
fn nats_greater_than_matches_one_or_more_trailing_tokens() {
    assert!(nats_subject_matches("a2a.gateway.>", "a2a.gateway.x.y"));
    assert!(nats_subject_matches("a2a.gateway.>", "a2a.gateway.x"));
    assert!(!nats_subject_matches("a2a.gateway.>", "a2a.gateway"));
}

#[test]
fn nats_pattern_mid_subject_greater_than_is_literal() {
    // `>` is only valid as the final token; a mid-pattern `>` should be
    // treated as a literal so it doesn't accidentally match real
    // subjects (no real subject token equals literal `>`).
    assert!(!nats_subject_matches("a2a.>.send", "a2a.gateway.send"));
}

#[test]
fn from_env_non_unicode_enable_flag_surfaces_typed_error() {
    // Inject `VarError::NotUnicode` for the enable flag so the typed
    // EnablementNotUnicode variant fires instead of being conflated with
    // a typo'd boolean value. Without this variant, an operator would
    // see a "not a recognized boolean" message that looked like a flag
    // name typo rather than a value encoding problem.
    let env = NotUnicodeForEnableFlag;
    let err = Tier1DeclarativeConfig::from_env(&env).expect_err("non-unicode rejected");
    assert!(matches!(err, Tier1DeclarativeBuildError::EnablementNotUnicode));
}

struct NotUnicodeForEnableFlag;

impl trogon_std::env::ReadEnv for NotUnicodeForEnableFlag {
    fn var(&self, name: &str) -> Result<String, std::env::VarError> {
        if name == ENV_TIER1_DECLARATIVE_ENABLED {
            return Err(std::env::VarError::NotUnicode(std::ffi::OsString::new()));
        }
        Err(std::env::VarError::NotPresent)
    }
}

#[test]
fn from_env_trims_bundle_dir_whitespace_before_load() {
    // Operators routinely set env vars from shell scripts that leave
    // trailing whitespace; without trimming, the loader would `exists()`
    // a path like `/path/to/dir ` and silently return an empty bundle.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("allow.tier1.toml"),
        r#"
            [[rule]]
            id = "always-allow"
            priority = 1
            effect = "allow"

            [[rule.matches]]
            kind = "agent_id"
            pattern = "*"
        "#,
    )
    .expect("write bundle");
    let padded = format!("  {}  ", dir.path().to_str().expect("path utf8"));
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENV_TIER1_DECLARATIVE_ENABLED, "on");
    env.set(ENV_TIER1_BUNDLE_DIR, padded);
    let layer = Tier1DeclarativeConfig::from_env(&env).expect("padded path resolves");
    assert!(layer.gate.is_enabled());
}
