use crate::policy::RuleName;
use crate::policy::tier2::{Tier2Decision, Tier2EvaluationContext};
use crate::policy::tier3_redaction::{Tier3EvaluationContext, Tier3RedactionDecision};

use super::*;

#[test]
fn enabled_redaction_without_a_bundle_keeps_payloads_intact() {
    for bundle in [None, Some(" \n ")] {
        let env = InMemoryEnv::new();
        env.set(ENV_TIER3_REDACTION_ENABLED, " On ");
        if let Some(bundle) = bundle {
            env.set(ENV_POLICY_BUNDLE_DIR, bundle);
        }
        let stack = gateway_policy_stack_from_env(&env);
        let payload = serde_json::json!({"private": "unchanged"});
        let mut context = Tier3EvaluationContext::new("message/send", None, payload.clone(), BTreeMap::new());
        assert_eq!(
            stack.tier3_gate.redact(&mut context),
            Tier3RedactionDecision::Allow { rewrites: Vec::new() }
        );
        assert_eq!(context.payload(), &payload);
        assert!(stack.substrate.is_none());
    }
}

#[test]
fn configured_cel_rules_control_ingress_and_broken_bundles_deny() {
    let root = tempfile::tempdir().unwrap();
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, root.path().to_string_lossy());
    env.set("A2A_GATEWAY_TIER2_CEL_ENABLED", "true");
    let context = Tier2EvaluationContext::new(
        a2a_nats::A2aMethod::MessageSend,
        serde_json::json!({}),
        None,
        a2a_nats::A2aAgentId::new("planner").unwrap(),
        None,
        BTreeMap::new(),
    );
    let evaluate = || {
        let stack = gateway_policy_stack_from_env(&env);
        stack.substrate.unwrap().tier2.evaluator().unwrap().evaluate(&context)
    };
    assert_eq!(evaluate(), Tier2Decision::Allow);
    std::fs::write(root.path().join("tier2"), "not a directory").unwrap();
    assert_eq!(
        evaluate(),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
    std::fs::remove_file(root.path().join("tier2")).unwrap();
    std::fs::create_dir(root.path().join("tier2")).unwrap();
    let rule = root.path().join("tier2/access.cel");
    std::fs::write(&rule, "true").unwrap();
    assert_eq!(evaluate(), Tier2Decision::Allow);
    std::fs::write(&rule, "false").unwrap();
    assert_eq!(
        evaluate(),
        Tier2Decision::Deny {
            rule: RuleName::new("access").unwrap()
        }
    );
    std::fs::write(&rule, "(broken").unwrap();
    assert_eq!(
        evaluate(),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn configured_skills_load_usable_bundles_and_skip_invalid_or_missing_entries() {
    let root = tempfile::tempdir().unwrap();
    let bundle = WasmBundlePath::new(root.path());
    let skill = SkillId::new("identity").unwrap();
    let module = bundle.join_skill_wasm(&skill);
    std::fs::create_dir_all(module.parent().unwrap()).unwrap();
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../a2a-redaction/tests/fixtures/identity_redact_part.wasm"),
        &module,
    )
    .unwrap();
    std::fs::write(bundle.join_skill_manifest(&skill), br#"{"json_path":"$.message"}"#).unwrap();
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, root.path().to_string_lossy());
    env.set(ENV_POLICY_SKILLS, " , ../invalid, missing, identity, ");
    env.set(ENV_TIER3_REDACTION_ENABLED, "true");
    let stack = gateway_policy_stack_from_env(&env);
    assert_eq!(stack.tier3_manifests.keys().collect::<Vec<_>>(), vec![&skill]);
    let payload = serde_json::json!({"message": "preserved by the guest"});
    let mut context = Tier3EvaluationContext::new("message/send", None, payload.clone(), stack.tier3_manifests);
    assert_eq!(
        stack.tier3_gate.redact(&mut context),
        Tier3RedactionDecision::Allow { rewrites: Vec::new() }
    );
    assert_eq!(context.payload(), &payload);
}
