use super::*;

fn req(tenant: &str, agent: &str, skill: &str) -> PerSkillRequest {
    PerSkillRequest {
        tenant: tenant.into(),
        agent: agent.into(),
        skill: SkillId::new(skill).expect("non-empty test skill id"),
    }
}

#[test]
fn skill_scope_overrides_agent_scope() {
    let mut p = PerSkillPolicy::new(PerSkillPolicyConfig::default());
    p.upsert(
        SkillScope::Agent {
            tenant: "acme".into(),
            agent: "planner".into(),
        },
        SkillRule::allow("agent-wide"),
    );
    p.upsert(
        SkillScope::Skill {
            tenant: "acme".into(),
            agent: "planner".into(),
            skill: SkillId::new("search").expect("non-empty test skill id"),
        },
        SkillRule::deny("skill-block"),
    );
    let decision = p.evaluate(&req("acme", "planner", "search"));
    assert!(matches!(
        decision,
        PerSkillDecision::Deny { ref reason, .. } if reason == "skill-block"
    ));
}

#[test]
fn tenant_scope_applies_when_no_finer_match() {
    let mut p = PerSkillPolicy::new(PerSkillPolicyConfig::default());
    p.upsert(
        SkillScope::Tenant { tenant: "acme".into() },
        SkillRule::allow("tenant-default"),
    );
    let decision = p.evaluate(&req("acme", "planner", "search"));
    assert!(matches!(decision, PerSkillDecision::Allow { .. }));
}

#[test]
fn default_deny_when_no_bundles() {
    let p = PerSkillPolicy::new(PerSkillPolicyConfig::default());
    let decision = p.evaluate(&req("acme", "planner", "search"));
    assert!(matches!(
        decision,
        PerSkillDecision::Deny { ref reason, .. } if reason == "no_policy"
    ));
}

#[test]
fn default_allow_when_disabled() {
    let p = PerSkillPolicy::new(PerSkillPolicyConfig {
        enforce: true,
        default_deny_when_no_bundles: false,
    });
    let decision = p.evaluate(&req("acme", "planner", "search"));
    assert!(matches!(decision, PerSkillDecision::Allow { contributor: None }));
}

#[test]
fn shadow_mode_wraps_would_be_decision() {
    let mut p = PerSkillPolicy::new(PerSkillPolicyConfig {
        enforce: false,
        default_deny_when_no_bundles: true,
    });
    p.upsert(
        SkillScope::Skill {
            tenant: "acme".into(),
            agent: "planner".into(),
            skill: SkillId::new("search").expect("non-empty test skill id"),
        },
        SkillRule::deny("forbidden"),
    );
    let decision = p.evaluate(&req("acme", "planner", "search"));
    match decision {
        PerSkillDecision::Shadow { would_be } => {
            assert!(matches!(
                *would_be,
                PerSkillDecision::Deny { ref reason, .. } if reason == "forbidden"
            ));
        }
        other => panic!("expected Shadow, got {other:?}"),
    }
}

#[test]
fn outcome_label_unwraps_shadow() {
    let inner = PerSkillDecision::Deny {
        reason: "x".into(),
        contributor: None,
    };
    let shadow = PerSkillDecision::Shadow {
        would_be: Box::new(inner),
    };
    assert_eq!(shadow.outcome(), "deny");
}
