use super::*;
use crate::decision_terminal::DecisionTerminal;

#[test]
fn enforcement_follows_the_outcome() {
    assert_eq!(
        DecisionOutcome::Allow(Enforcement::Enforce).enforcement(),
        Enforcement::Enforce
    );
    assert_eq!(
        DecisionOutcome::Allow(Enforcement::DryRun).enforcement(),
        Enforcement::DryRun
    );
    assert_eq!(DecisionOutcome::Deny.enforcement(), Enforcement::Enforce);
    assert_eq!(DecisionOutcome::WouldDeny.enforcement(), Enforcement::DryRun);
}

#[test]
fn terminal_follows_the_outcome() {
    assert_eq!(
        DecisionOutcome::Allow(Enforcement::DryRun).terminal(),
        DecisionTerminal::Allow
    );
    assert_eq!(DecisionOutcome::Deny.terminal(), DecisionTerminal::Deny);
    assert_eq!(DecisionOutcome::WouldDeny.terminal(), DecisionTerminal::WouldDeny);
}

#[test]
fn only_an_enforced_deny_is_a_refusal() {
    assert!(DecisionOutcome::Deny.is_refusal());
    assert!(!DecisionOutcome::WouldDeny.is_refusal());
    assert!(!DecisionOutcome::Allow(Enforcement::Enforce).is_refusal());
}
