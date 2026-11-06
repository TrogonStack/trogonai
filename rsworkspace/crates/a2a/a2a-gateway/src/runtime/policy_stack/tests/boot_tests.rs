use crate::policy::RuleName;
use crate::policy::tier2::{Tier2Decision, Tier2EvaluationContext};
use crate::policy::tier3_redaction::{Tier3EvaluationContext, Tier3RedactionDecision};

use super::*;

fn tier2_env(root: &std::path::Path) -> InMemoryEnv {
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, root.to_string_lossy());
    env.set(ENV_GATEWAY_TIER2_CEL_ENABLED, "true");
    env
}

fn skills_env(root: &std::path::Path, skills: &str) -> InMemoryEnv {
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, root.to_string_lossy());
    env.set(ENV_POLICY_SKILLS, skills);
    env
}

fn install_identity_skill(root: &std::path::Path) -> SkillId {
    let bundle = WasmBundlePath::new(root);
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
    skill
}

#[test]
fn enabled_redaction_with_a_bundle_dir_boots_a_real_gate() {
    let root = tempfile::tempdir().unwrap();
    let env = InMemoryEnv::new();
    env.set(ENV_TIER3_REDACTION_ENABLED, " On ");
    env.set(ENV_POLICY_BUNDLE_DIR, root.path().to_string_lossy());
    let stack = boot_stack(&env);
    let payload = serde_json::json!({"private": "unchanged"});
    let mut context = Tier3EvaluationContext::new("message/send", None, payload.clone(), BTreeMap::new());
    assert_eq!(
        stack.tier3_gate.redact(&mut context),
        Tier3RedactionDecision::Allow { rewrites: Vec::new() }
    );
    assert_eq!(context.payload(), &payload);
    assert!(stack.substrate.is_some());
}

#[test]
fn from_env_rejects_an_unusable_bundle_directory() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    std::fs::write(&file, "not a directory").unwrap();
    for path in [root.path().join("missing"), file] {
        for redaction in [false, true] {
            let env = InMemoryEnv::new();
            env.set(ENV_POLICY_BUNDLE_DIR, path.to_string_lossy());
            if redaction {
                env.set(ENV_TIER3_REDACTION_ENABLED, "true");
            }
            let error = boot_error(&env);
            assert!(matches!(
                &error,
                PolicyStackError::BundleDirectory { bundle_dir, .. } if bundle_dir == &path
            ));
            assert!(std::error::Error::source(&error).is_some());
        }
    }
}

#[test]
fn configured_cel_rules_control_ingress_and_broken_reloads_deny() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("tier2")).unwrap();
    let rule = root.path().join("tier2/access.cel");
    std::fs::write(&rule, "true").unwrap();
    let env = tier2_env(root.path());
    let context = Tier2EvaluationContext::new(
        a2a_nats::A2aMethod::MessageSend,
        serde_json::json!({}),
        None,
        a2a_nats::A2aAgentId::new("planner").unwrap(),
        None,
        BTreeMap::new(),
    );
    let stack = boot_stack(&env);
    let substrate = stack.substrate.unwrap();
    let evaluate = || substrate.tier2.evaluator().unwrap().evaluate(&context);
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
fn from_env_rejects_tier2_enabled_with_missing_tier2_dir() {
    let root = tempfile::tempdir().unwrap();
    let env = tier2_env(root.path());
    assert!(matches!(
        boot_error(&env),
        PolicyStackError::Tier2NoRules { tier2_dir } if tier2_dir == root.path().join("tier2")
    ));
}

#[test]
fn from_env_rejects_tier2_enabled_with_zero_rules() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("tier2")).unwrap();
    std::fs::write(root.path().join("tier2/notes.txt"), "not a rule").unwrap();
    let env = tier2_env(root.path());
    assert!(matches!(boot_error(&env), PolicyStackError::Tier2NoRules { .. }));
}

#[test]
fn from_env_rejects_tier2_enabled_when_tier2_path_is_not_a_directory() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("tier2"), "not a directory").unwrap();
    let env = tier2_env(root.path());
    assert!(matches!(
        boot_error(&env),
        PolicyStackError::Tier2Load { tier2_dir, .. } if tier2_dir == root.path().join("tier2")
    ));
}

#[test]
fn from_env_rejects_tier2_enabled_when_a_rule_fails_to_compile() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("tier2")).unwrap();
    std::fs::write(root.path().join("tier2/broken.cel"), "(broken").unwrap();
    let env = tier2_env(root.path());
    assert!(matches!(boot_error(&env), PolicyStackError::Tier2Load { .. }));
}

#[test]
fn from_env_ignores_the_tier2_dir_when_tier2_is_off() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("tier2"), "not a directory").unwrap();
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, root.path().to_string_lossy());
    assert!(boot_stack(&env).substrate.is_some());
}

#[test]
fn configured_skills_load_usable_bundles() {
    let root = tempfile::tempdir().unwrap();
    let skill = install_identity_skill(root.path());
    let env = skills_env(root.path(), " , identity, ");
    env.set(ENV_TIER3_REDACTION_ENABLED, "true");
    let stack = boot_stack(&env);
    assert_eq!(stack.tier3_manifests.keys().collect::<Vec<_>>(), vec![&skill]);
    let payload = serde_json::json!({"message": "preserved by the guest"});
    let mut context = Tier3EvaluationContext::new("message/send", None, payload.clone(), stack.tier3_manifests);
    assert_eq!(
        stack.tier3_gate.redact(&mut context),
        Tier3RedactionDecision::Allow { rewrites: Vec::new() }
    );
    assert_eq!(context.payload(), &payload);
}

#[test]
fn from_env_rejects_an_invalid_skill_slug() {
    let root = tempfile::tempdir().unwrap();
    install_identity_skill(root.path());
    let env = skills_env(root.path(), "identity, ../invalid");
    assert!(matches!(
        boot_error(&env),
        PolicyStackError::InvalidSkillSlug { slug, .. } if slug == "../invalid"
    ));
}

#[test]
fn from_env_rejects_a_skill_that_fails_to_preload() {
    let root = tempfile::tempdir().unwrap();
    let env = skills_env(root.path(), "missing");
    assert!(matches!(
        boot_error(&env),
        PolicyStackError::SkillPreload { slug, .. } if slug == "missing"
    ));
}
