use trogon_std::env::InMemoryEnv;

use super::*;

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

#[test]
fn from_env_returns_noop_when_bundle_dir_unset() {
    // Without a configured bundle dir the boot helper must hand back
    // a Noop stack rather than panicking or constructing a phantom
    // substrate that would later fail every redact call.
    let env = InMemoryEnv::new();
    let stack = gateway_policy_stack_from_env(&env);
    assert!(stack.substrate.is_none());
    assert!(stack.tier3_manifests.is_empty());
}

#[test]
fn from_env_returns_noop_when_bundle_dir_blank() {
    // Operators that clear the env var to "disable" the bundle
    // (leaving whitespace) get the same Noop fallback as an absent
    // key.
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, "   ");
    let stack = gateway_policy_stack_from_env(&env);
    assert!(stack.substrate.is_none());
}

#[test]
fn from_env_returns_substrate_when_bundle_dir_set_to_any_path() {
    // The Wasmtime substrate's constructor is lazy w.r.t. file
    // existence: it builds the host without reading the bundle dir
    // up front. With a configured bundle dir the boot helper hands
    // back a stack carrying a substrate, even when no per-skill
    // bundles are preloaded -- per-skill load happens later via
    // `A2A_GATEWAY_POLICY_SKILLS`. Tier-3 redaction stays disabled
    // because `A2A_GATEWAY_TIER3_REDACTION_ENABLED` is unset.
    let env = InMemoryEnv::new();
    env.set(ENV_POLICY_BUNDLE_DIR, "/tmp/policy-bundle-for-boot-test");
    let stack = gateway_policy_stack_from_env(&env);
    assert!(stack.substrate.is_some());
    assert!(stack.tier3_manifests.is_empty());
}
