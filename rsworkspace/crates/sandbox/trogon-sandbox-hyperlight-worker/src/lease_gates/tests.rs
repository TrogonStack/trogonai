use std::sync::Arc;

use super::*;

#[test]
fn the_same_lease_shares_one_gate() {
    let gates = LeaseGates::default();
    let first = gates.gate(&LeaseId::new("a"));
    let second = gates.gate(&LeaseId::new("a"));
    let other = gates.gate(&LeaseId::new("b"));
    assert!(Arc::ptr_eq(&first, &second));
    assert!(!Arc::ptr_eq(&first, &other));
    assert_eq!(gates.len(), 2);
}

#[test]
fn a_gate_nobody_holds_is_pruned() {
    let gates = LeaseGates::default();
    drop(gates.gate(&LeaseId::new("a")));
    let kept = gates.gate(&LeaseId::new("b"));
    assert_eq!(gates.len(), 1);
    drop(kept);
}

#[test]
fn a_poisoned_registry_keeps_working() {
    let gates = Arc::new(LeaseGates::default());
    let poisoner = Arc::clone(&gates);
    let _ = std::thread::spawn(move || {
        let _held = poisoner.0.lock();
        std::panic::resume_unwind(Box::new("poisoned"));
    })
    .join();
    drop(gates.gate(&LeaseId::new("a")));
    assert_eq!(gates.len(), 1);
}
