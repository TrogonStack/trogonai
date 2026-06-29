use super::*;

#[test]
fn noop_stack_has_no_substrate() {
    // The substrate field is `None` (rather than `Some(noop_substrate)`)
    // so the dispatch path can skip the Tier-2 CEL step entirely
    // rather than paying for an evaluator call that would always
    // return Allow anyway.
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
    // The gate is always present (Noop when disabled) so dispatch
    // never has to branch on "do we have a gate at all", only on
    // the decision the gate returns.
    let stack = GatewayPolicyStack::noop();
    // Smoke check: the trait object answers to a method call. The
    // actual Allow/Refuse decision is exercised by the gate's own
    // tests; here we only assert the field is wired.
    let _ = stack.tier3_gate.as_ref();
}
