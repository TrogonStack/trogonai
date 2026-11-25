use a2a_gateway::policy::tier3_redaction::{
    Tier3EvaluationContext, Tier3ManifestLoadError, Tier3RedactionDecision, Tier3SkillManifestError,
    load_tier3_manifest_from_bundle,
};
use a2a_gateway::runtime::policy_stack::{PolicyStackError, gateway_policy_stack_from_env};
use a2a_redaction::{SkillId, WasmBundlePath};
use trogon_std::env::InMemoryEnv;

fn skill_bundle() -> Result<(tempfile::TempDir, WasmBundlePath, SkillId, InMemoryEnv), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let bundle = WasmBundlePath::new(root.path());
    let skill = SkillId::new("identity")?;
    let wasm = bundle.join_skill_wasm(&skill);
    std::fs::create_dir_all(wasm.parent().ok_or("skill module must have a parent")?)?;
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../a2a-redaction/tests/fixtures/identity_redact_part.wasm"),
        wasm,
    )?;
    let env = InMemoryEnv::new();
    env.set("A2A_GATEWAY_POLICY_BUNDLE_DIR", root.path().to_string_lossy());
    env.set("A2A_GATEWAY_POLICY_SKILLS", "identity");
    Ok((root, bundle, skill, env))
}

#[test]
fn enabled_tier3_rejects_unusable_manifests() -> Result<(), Box<dyn std::error::Error>> {
    for manifest in [
        "{",
        "{}",
        r#"{"json_path":7}"#,
        r#"{"json_path":""}"#,
        r#"{"json_path":"   "}"#,
        r#"{"json_path":"$.."}"#,
    ] {
        let (_root, bundle, skill, env) = skill_bundle()?;
        std::fs::write(bundle.join_skill_manifest(&skill), manifest)?;
        env.set("A2A_GATEWAY_TIER3_REDACTION_ENABLED", "true");
        let error = gateway_policy_stack_from_env(&env)
            .err()
            .ok_or("enabled boot must fail")?;
        assert!(std::error::Error::source(&error).is_some());
        let PolicyStackError::Tier3Manifest { skill_id, source } = error else {
            return Err("expected a typed manifest boot failure".into());
        };
        assert_eq!(skill_id, skill);
        assert!(std::error::Error::source(&source).is_some());
        match manifest {
            "{" => assert!(matches!(source, Tier3ManifestLoadError::Json(_))),
            "{}" | r#"{"json_path":7}"# => assert!(matches!(
                source,
                Tier3ManifestLoadError::Manifest(Tier3SkillManifestError::MissingJsonPath)
            )),
            r#"{"json_path":""}"# | r#"{"json_path":"   "}"# => assert!(matches!(
                source,
                Tier3ManifestLoadError::Manifest(Tier3SkillManifestError::EmptyJsonPath)
            )),
            _ => assert!(matches!(
                source,
                Tier3ManifestLoadError::Manifest(Tier3SkillManifestError::InvalidJsonPath(_))
            )),
        }
    }
    Ok(())
}

#[test]
fn disabled_tier3_preserves_manifest_skip_behavior() -> Result<(), Box<dyn std::error::Error>> {
    for manifest in ["{", "{}", r#"{"json_path":"$.."}"#] {
        let (_root, bundle, skill, env) = skill_bundle()?;
        std::fs::write(bundle.join_skill_manifest(&skill), manifest)?;
        let stack = gateway_policy_stack_from_env(&env)?;
        assert!(stack.substrate.is_some());
        assert!(stack.tier3_manifests.is_empty());
    }
    Ok(())
}

#[test]
fn missing_and_unreadable_manifests_preserve_preload_failure() -> Result<(), Box<dyn std::error::Error>> {
    for directory in [false, true] {
        let (_root, bundle, skill, env) = skill_bundle()?;
        if directory {
            std::fs::create_dir(bundle.join_skill_manifest(&skill))?;
        }
        assert!(matches!(
            load_tier3_manifest_from_bundle(&bundle, &skill),
            Err(Tier3ManifestLoadError::Read(_))
        ));
        for enabled in ["true", "false"] {
            env.set("A2A_GATEWAY_TIER3_REDACTION_ENABLED", enabled);
            assert!(matches!(gateway_policy_stack_from_env(&env),
                Err(PolicyStackError::SkillPreload { slug, .. }) if slug == skill.as_str()));
        }
    }
    Ok(())
}

#[test]
fn enabled_tier3_rejects_non_utf8_manifest_and_disabled_skips_it() -> Result<(), Box<dyn std::error::Error>> {
    let (_root, bundle, skill, env) = skill_bundle()?;
    std::fs::write(bundle.join_skill_manifest(&skill), [0xff])?;
    env.set("A2A_GATEWAY_TIER3_REDACTION_ENABLED", "true");
    assert!(matches!(gateway_policy_stack_from_env(&env),
        Err(PolicyStackError::Tier3Manifest { skill_id, source: Tier3ManifestLoadError::Read(source) })
        if skill_id == skill && source.kind() == std::io::ErrorKind::InvalidData));
    env.set("A2A_GATEWAY_TIER3_REDACTION_ENABLED", "false");
    assert!(gateway_policy_stack_from_env(&env)?.tier3_manifests.is_empty());
    Ok(())
}

#[test]
fn valid_manifest_executes_the_real_gate_only_when_enabled() -> Result<(), Box<dyn std::error::Error>> {
    let (_root, bundle, skill, env) = skill_bundle()?;
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../a2a-redaction/tests/fixtures/tier3_refuse.wasm"),
        bundle.join_skill_wasm(&skill),
    )?;
    std::fs::write(
        bundle.join_skill_manifest(&skill),
        r#"{"skill_id":"other","json_path":"$.message","kind":"unknown"}"#,
    )?;
    for enabled in ["true", "false"] {
        env.set("A2A_GATEWAY_TIER3_REDACTION_ENABLED", enabled);
        let stack = gateway_policy_stack_from_env(&env)?;
        assert_eq!(stack.tier3_manifests.keys().collect::<Vec<_>>(), vec![&skill]);
        let payload = serde_json::json!({"message":"private"});
        let mut context = Tier3EvaluationContext::new("message/send", None, payload.clone(), stack.tier3_manifests);
        let decision = stack.tier3_gate.redact(&mut context);
        if enabled == "true" {
            assert!(matches!(decision, Tier3RedactionDecision::Refuse { rule, .. } if rule == skill));
        } else {
            assert_eq!(decision, Tier3RedactionDecision::Allow { rewrites: Vec::new() });
        }
        assert_eq!(context.payload(), &payload);
    }
    Ok(())
}
