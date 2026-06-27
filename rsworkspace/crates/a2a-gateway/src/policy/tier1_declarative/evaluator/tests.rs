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

fn rule(
    id: &str,
    priority: u32,
    effect: Tier1DeclarativeEffect,
    matches: Vec<Tier1DeclarativeMatch>,
) -> Tier1DeclarativeRule {
    Tier1DeclarativeRule {
        id: Tier1DeclarativeRuleId::new(id),
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
            Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentId, "planner", false),
            Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentMethod, "message/send", false),
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
            rule: Tier1DeclarativeRuleId::new("deny-planner")
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
            vec![Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentId, "other", false)],
        ),
        rule(
            "allow-planner",
            50,
            Tier1DeclarativeEffect::Allow,
            vec![Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentId, "planner", false)],
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
            rule: Some(Tier1DeclarativeRuleId::new("allow-planner"))
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
            vec![Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentId, "planner", false)],
        ),
        rule(
            "deny-high",
            100,
            Tier1DeclarativeEffect::Deny,
            vec![Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentId, "planner", false)],
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
        Tier1DeclarativeDecision::Deny {
            rule: Tier1DeclarativeRuleId::new("deny-high")
        }
    );
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
        vec![Tier1DeclarativeMatch::new(
            Tier1ResourceKind::CallerSubject,
            "user/alice",
            true,
        )],
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
            rule: Tier1DeclarativeRuleId::new("deny-non-alice")
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
        tier1_declarative_audit_rule_fired(&Tier1DeclarativeDecision::Allow {
            rule: Some(Tier1DeclarativeRuleId::new("ok"))
        }),
        "gateway.tier1.declarative.allowed.ok"
    );
    assert_eq!(
        tier1_declarative_audit_rule_fired(&Tier1DeclarativeDecision::Deny {
            rule: Tier1DeclarativeRuleId::new("blocked")
        }),
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
    assert!(matches!(err, Tier1DeclarativeBuildError::InvalidBundleDir(_)));
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
    let id = Tier1DeclarativeRuleId::new("rule-1");
    assert_eq!(format!("{id}"), "rule-1");
    assert_eq!(id.as_str(), "rule-1");
}

#[test]
fn fixed_clock_returns_configured_instant() {
    let now = std::time::SystemTime::now();
    let clock = FixedTier1Clock::new(now);
    assert_eq!(clock.now(), now);
}
