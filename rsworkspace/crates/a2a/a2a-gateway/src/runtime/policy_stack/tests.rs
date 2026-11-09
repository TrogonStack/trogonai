use trogon_std::env::InMemoryEnv;

use super::*;

mod boot_tests;

#[test]
fn noop_stack_has_no_substrate() {
    let stack = GatewayPolicyStack::noop();
    assert!(stack.substrate.is_none());
}

#[test]
fn noop_stack_has_empty_skill_manifests() {
    let stack = GatewayPolicyStack::noop();
    assert!(stack.tier3_manifests.is_empty());
}

#[test]
fn noop_stack_carries_a_redaction_gate() {
    let stack = GatewayPolicyStack::noop();
    let _ = stack.tier3_gate.as_ref();
}

fn boot_error<E: ReadEnv>(env: &E) -> PolicyStackError {
    match gateway_policy_stack_from_env(env) {
        Err(error) => error,
        Ok(_) => panic!("expected the policy stack to refuse to boot"),
    }
}

fn boot_stack<E: ReadEnv>(env: &E) -> GatewayPolicyStack {
    match gateway_policy_stack_from_env(env) {
        Ok(stack) => stack,
        Err(error) => panic!("expected the policy stack to boot: {error}"),
    }
}

#[test]
fn from_env_returns_noop_when_bundle_dir_unset() {
    // With nothing turned on and no bundle dir the boot helper hands
    // back a Noop stack rather than constructing a phantom substrate
    // that would later fail every redact call.
    let env = InMemoryEnv::new();
    let stack = boot_stack(&env);
    assert!(stack.substrate.is_none());
    assert!(stack.tier3_manifests.is_empty());
}

#[test]
fn from_env_returns_noop_when_bundle_dir_blank() {
    // Operators that clear the env var to "disable" the bundle
    // (leaving whitespace) get the same Noop stack as an absent key.
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, "   ");
    let stack = boot_stack(&env);
    assert!(stack.substrate.is_none());
}

#[test]
fn from_env_returns_noop_when_layers_are_explicitly_off() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_TIER2_CEL_ENABLED, "false");
    env.set(ENV_TIER3_REDACTION_ENABLED, "off");
    let stack = boot_stack(&env);
    assert!(stack.substrate.is_none());
}

#[test]
fn from_env_returns_substrate_when_bundle_dir_is_a_real_directory() {
    // With a configured bundle dir the boot helper hands back a stack
    // carrying a substrate, even when no per-skill bundles are
    // preloaded -- per-skill load happens via
    // `A2A_GATEWAY_POLICY_SKILLS`. Tier-3 redaction stays disabled
    // because `A2A_GATEWAY_TIER3_REDACTION_ENABLED` is unset.
    let root = tempfile::tempdir().unwrap();
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, root.path().to_string_lossy());
    let stack = boot_stack(&env);
    assert!(stack.substrate.is_some());
    assert!(stack.tier3_manifests.is_empty());
}

#[test]
fn from_env_rejects_unusable_signing_pubkey() {
    // A bundle dir plus a mistyped signing pubkey must not boot a
    // substrate that would execute those bundles unverified.
    let root = tempfile::tempdir().unwrap();
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, root.path().to_string_lossy());
    env.set(ENV_GATEWAY_TIER3_SIGNING_PUBKEY, "not-hex");
    assert!(matches!(boot_error(&env), PolicyStackError::InvalidSigningKey));
}

#[test]
fn from_env_returns_substrate_when_signing_pubkey_is_valid() {
    let root = tempfile::tempdir().unwrap();
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, root.path().to_string_lossy());
    env.set(
        ENV_GATEWAY_TIER3_SIGNING_PUBKEY,
        "abababababababababababababababababababababababababababababababab",
    );
    let stack = boot_stack(&env);
    assert!(stack.substrate.is_some());
}

#[test]
fn from_env_rejects_tier2_enabled_without_bundle_dir() {
    for bundle in [None, Some("  ")] {
        let env = InMemoryEnv::new();
        env.set(ENV_GATEWAY_TIER2_CEL_ENABLED, "true");
        if let Some(bundle) = bundle {
            env.set(ENV_POLICY_BUNDLE_DIR, bundle);
        }
        assert!(matches!(
            boot_error(&env),
            PolicyStackError::BundleDirRequired {
                enabled_by: ENV_GATEWAY_TIER2_CEL_ENABLED
            }
        ));
    }
}

#[test]
fn from_env_rejects_tier3_enabled_without_bundle_dir() {
    for bundle in [None, Some("  ")] {
        let env = InMemoryEnv::new();
        env.set(ENV_TIER3_REDACTION_ENABLED, " On ");
        if let Some(bundle) = bundle {
            env.set(ENV_POLICY_BUNDLE_DIR, bundle);
        }
        assert!(matches!(
            boot_error(&env),
            PolicyStackError::BundleDirRequired {
                enabled_by: ENV_TIER3_REDACTION_ENABLED
            }
        ));
    }
}

#[test]
fn policy_stack_error_names_the_enabling_setting() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER3_REDACTION_ENABLED, "true");
    assert!(matches!(
        boot_error(&env),
        PolicyStackError::BundleDirRequired {
            enabled_by: ENV_TIER3_REDACTION_ENABLED
        }
    ));
}
