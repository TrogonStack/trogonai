#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use common::{
    agent, chain, chain_of_a, egress, egress_permission, enforce, fresh_now, hold, model_ceiling, model_use, node,
    rule_ref, view,
};
use trogon_policy_eval::{
    Decision, Effective, HoldName, HoldScope, Narrowing, Phase, PolicySnapshot, Refusal, Request, RuleKind, RuleName,
};

fn rule(name: &str) -> RuleName {
    RuleName::new(name).expect("valid rule name")
}

fn decide(snapshot: PolicySnapshot, request: &Request, narrowing: &[Narrowing]) -> Decision {
    enforce().evaluate(&view(snapshot), request, narrowing, fresh_now())
}

/// A request that has no permit, a hold, a ceiling exclusion and a denying rule
/// at the same time, so that every refusal kind competes for the answer.
fn everything_refuses() -> (PolicySnapshot, Request) {
    let snapshot = common::builder()
        .holds(node("team"), 2, [hold("stop", HoldScope::AllProtectedActions)])
        .model_ceiling(node("org"), 3, model_ceiling(&["n"]))
        .build();
    let request = model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction);
    (snapshot, request)
}

#[test]
fn a_request_rule_cannot_stand_in_for_a_missing_permit() {
    let request = egress(chain_of_a(), "example.com", 443);
    let decision = decide(common::empty_snapshot(), &request, &[Narrowing::Pass, Narrowing::Pass]);
    assert_eq!(
        decision.would_deny(),
        Some(&Refusal::NoPermit(RuleKind::EgressPermission))
    );
}

#[test]
fn a_request_rule_that_passes_changes_nothing() {
    let snapshot = common::builder()
        .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
        .build();
    let request = egress(chain_of_a(), "example.com", 443);
    let without = decide(snapshot.clone(), &request, &[]);
    let with = decide(snapshot, &request, &[Narrowing::Pass]);
    assert_eq!(without, with);
    assert_eq!(with.effective(), &Effective::Admit);
}

#[test]
fn a_request_rule_that_denies_narrows_an_admitted_request() {
    let snapshot = common::builder()
        .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
        .build();
    let request = egress(chain_of_a(), "example.com", 443);
    let decision = decide(
        snapshot,
        &request,
        &[Narrowing::Pass, Narrowing::Deny(rule("no-exfil"))],
    );
    assert_eq!(
        decision.effective(),
        &Effective::Refuse(Refusal::RequestRuleDenied(rule("no-exfil")))
    );
}

#[test]
fn every_refusal_names_exactly_one_binding_by_a_fixed_order() {
    let (snapshot, request) = everything_refuses();
    let decision = decide(snapshot, &request, &[Narrowing::Deny(rule("rule"))]);
    // The hold and the ceiling both refuse. The hold sorts first in the derived
    // order; this is determinism, not precedence, and the ceiling is still read.
    assert_eq!(
        decision.would_deny(),
        Some(&Refusal::HoldPlaced {
            rule: rule_ref(RuleKind::Hold, node("team"), 2),
            hold: HoldName::new("stop").expect("valid"),
        })
    );
    assert!(
        decision
            .consulted()
            .iter()
            .any(|rule| rule.kind() == RuleKind::ModelCeiling && rule.subject() == &node("org"))
    );
}

#[test]
fn request_rules_are_only_asked_after_the_bindings_admit() {
    let (snapshot, request) = everything_refuses();
    let with_rule = decide(snapshot.clone(), &request, &[Narrowing::Deny(rule("rule"))]);
    let without_rule = decide(snapshot, &request, &[]);
    assert_eq!(with_rule, without_rule);
}

#[test]
fn several_denying_rules_report_the_smallest_name_whatever_their_order() {
    let snapshot = common::builder()
        .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
        .build();
    let request = egress(chain_of_a(), "example.com", 443);
    let forward = [
        Narrowing::Deny(rule("alpha")),
        Narrowing::Deny(rule("beta")),
        Narrowing::Pass,
    ];
    let backward = [
        Narrowing::Pass,
        Narrowing::Deny(rule("beta")),
        Narrowing::Deny(rule("alpha")),
    ];
    let expected = Some(&Refusal::RequestRuleDenied(rule("alpha")));
    assert_eq!(decide(snapshot.clone(), &request, &forward).would_deny(), expected);
    assert_eq!(decide(snapshot, &request, &backward).would_deny(), expected);
}

#[test]
fn the_answer_does_not_depend_on_how_the_snapshot_was_assembled() {
    let forward = common::builder()
        .holds(node("team"), 2, [hold("stop", HoldScope::AllProtectedActions)])
        .holds(node("org"), 5, [hold("also", HoldScope::AllProtectedActions)])
        .model_ceiling(agent("a"), 1, model_ceiling(&["n"]))
        .model_ceiling(node("org"), 3, model_ceiling(&["n"]))
        .build();
    let backward = common::builder()
        .model_ceiling(node("org"), 3, model_ceiling(&["n"]))
        .model_ceiling(agent("a"), 1, model_ceiling(&["n"]))
        .holds(node("org"), 5, [hold("also", HoldScope::AllProtectedActions)])
        .holds(node("team"), 2, [hold("stop", HoldScope::AllProtectedActions)])
        .build();
    assert_eq!(forward, backward);

    let request = model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction);
    assert_eq!(decide(forward, &request, &[]), decide(backward, &request, &[]));
}

#[test]
fn the_answer_does_not_depend_on_the_order_of_the_chains() {
    let snapshot = common::builder()
        .holds(node("team"), 2, [hold("stop", HoldScope::AllProtectedActions)])
        .holds(node("org"), 5, [hold("also", HoldScope::AllProtectedActions)])
        .model_ceiling(node("team"), 3, model_ceiling(&["n"]))
        .model_ceiling(node("org"), 4, model_ceiling(&["n"]))
        .build();
    let permutations = [
        vec![agent("a"), node("team"), node("org")],
        vec![agent("a"), node("org"), node("team")],
        vec![node("org"), agent("a"), node("team")],
        vec![node("team"), node("org"), agent("a")],
        vec![node("org"), node("team"), agent("a")],
    ];
    let expected = decide(
        snapshot.clone(),
        &model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction),
        &[],
    );
    for subjects in permutations {
        let lineage = chain(subjects);
        let request = model_use(chain_of_a(), lineage, "m", Phase::ProtectedAction);
        assert_eq!(decide(snapshot.clone(), &request, &[]), expected);
    }
}

#[test]
fn consulted_lists_each_history_once_in_a_fixed_order() {
    let snapshot = common::builder()
        .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
        .holds(agent("a"), 2, [])
        .build();
    let request = egress(chain_of_a(), "example.com", 443);
    let decision = decide(snapshot, &request, &[]);
    let consulted = decision.consulted();

    let mut sorted = consulted.to_vec();
    sorted.sort();
    sorted.dedup();
    assert_eq!(consulted, sorted.as_slice());
    // An egress request reads the permit and the holds on each of 3 subjects.
    assert_eq!(consulted.len(), 6);
}

#[test]
fn a_model_request_does_not_read_permits() {
    let request = model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction);
    let decision = decide(common::empty_snapshot(), &request, &[]);
    assert!(
        decision
            .consulted()
            .iter()
            .all(|rule| rule.kind() != RuleKind::EgressPermission && rule.kind() != RuleKind::DelegationPermission)
    );
    // 3 ceilings and 3 hold histories.
    assert_eq!(decision.consulted().len(), 6);
}

#[test]
fn a_refusal_names_the_binding_in_its_text() {
    let (snapshot, request) = everything_refuses();
    let decision = decide(snapshot, &request, &[]);
    let text = decision.would_deny().expect("refused").to_string();
    assert!(text.contains("stop"), "{text}");
    assert!(text.contains("node team"), "{text}");
    assert!(text.contains("revision 2"), "{text}");

    let missing = decide(common::empty_snapshot(), &egress(chain_of_a(), "example.com", 443), &[]);
    let text = missing.would_deny().expect("refused").to_string();
    assert!(text.contains("egress permission"), "{text}");
}
