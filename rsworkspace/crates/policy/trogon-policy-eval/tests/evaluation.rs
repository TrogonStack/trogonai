#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use common::{
    agent, agent_id, chain, chain_of_a, delegate, delegation_permission, egress, egress_permission, enforce, fresh_now,
    hold, model_ceiling, model_use, node, node_id, rule_ref, view,
};
use trogon_policy_eval::{
    Callee, DelegationTarget, Effective, HoldName, HoldScope, Phase, PolicySnapshot, Refusal, Request, RuleKind,
};

struct Case {
    name: &'static str,
    snapshot: PolicySnapshot,
    request: Request,
    expected: Option<Refusal>,
}

fn run(cases: Vec<Case>) {
    for case in cases {
        let decision = enforce().evaluate(&view(case.snapshot), &case.request, &[], fresh_now());
        assert_eq!(decision.would_deny(), case.expected.as_ref(), "case: {}", case.name);
        let effective = match case.expected {
            Some(refusal) => Effective::Refuse(refusal),
            None => Effective::Admit,
        };
        assert_eq!(decision.effective(), &effective, "case: {}", case.name);
    }
}

fn hold_placed(subject: trogon_policy_eval::Subject, revision: u64, name: &str) -> Refusal {
    Refusal::HoldPlaced {
        rule: rule_ref(RuleKind::Hold, subject, revision),
        hold: HoldName::new(name).expect("valid hold name"),
    }
}

fn callee_b() -> Callee {
    Callee::new(agent_id("b"), [node_id("callee-team")])
}

fn a_delegating_to_b(phase: Phase) -> Request {
    delegate(chain_of_a(), chain_of_a(), callee_b(), phase)
}

#[test]
fn egress_is_admitted_only_when_a_permit_on_the_own_chain_covers_it() {
    let no_permit = Some(Refusal::NoPermit(RuleKind::EgressPermission));
    run(vec![
        Case {
            name: "no bindings at all is denial",
            snapshot: common::empty_snapshot(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: no_permit.clone(),
        },
        Case {
            name: "permit on the agent",
            snapshot: common::builder()
                .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: None,
        },
        Case {
            name: "permit inherited from the nearest node",
            snapshot: common::builder()
                .egress_permission(node("team"), 1, egress_permission(&[("example.com", 443)]))
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: None,
        },
        Case {
            name: "permit inherited from the farthest node",
            snapshot: common::builder()
                .egress_permission(node("org"), 4, egress_permission(&[("example.com", 443)]))
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: None,
        },
        Case {
            name: "permit for another host",
            snapshot: common::builder()
                .egress_permission(agent("a"), 1, egress_permission(&[("other.example", 443)]))
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: no_permit.clone(),
        },
        Case {
            name: "a subdomain is not the host",
            snapshot: common::builder()
                .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
                .build(),
            request: egress(chain_of_a(), "api.example.com", 443),
            expected: no_permit.clone(),
        },
        Case {
            name: "permit for another port",
            snapshot: common::builder()
                .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 80)]))
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: no_permit.clone(),
        },
        Case {
            name: "permit on a node outside the own chain",
            snapshot: common::builder()
                .egress_permission(node("elsewhere"), 1, egress_permission(&[("example.com", 443)]))
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: no_permit.clone(),
        },
        Case {
            name: "permit on a sibling agent",
            snapshot: common::builder()
                .egress_permission(agent("z"), 1, egress_permission(&[("example.com", 443)]))
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: no_permit.clone(),
        },
        Case {
            name: "a permit that names no destination permits nothing",
            snapshot: common::builder()
                .egress_permission(agent("a"), 3, egress_permission(&[]))
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: no_permit.clone(),
        },
        Case {
            name: "permits union across the chain: agent's own destination",
            snapshot: common::builder()
                .egress_permission(agent("a"), 1, egress_permission(&[("one.example", 443)]))
                .egress_permission(node("team"), 1, egress_permission(&[("two.example", 443)]))
                .build(),
            request: egress(chain_of_a(), "one.example", 443),
            expected: None,
        },
        Case {
            name: "permits union across the chain: the node's destination",
            snapshot: common::builder()
                .egress_permission(agent("a"), 1, egress_permission(&[("one.example", 443)]))
                .egress_permission(node("team"), 1, egress_permission(&[("two.example", 443)]))
                .build(),
            request: egress(chain_of_a(), "two.example", 443),
            expected: None,
        },
        Case {
            name: "a delegation permit is not an egress permit",
            snapshot: common::builder()
                .delegation_permission(
                    agent("a"),
                    1,
                    delegation_permission(vec![DelegationTarget::Agent(agent_id("b"))]),
                )
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: no_permit,
        },
    ]);
}

#[test]
fn a_model_without_a_ceiling_is_not_capped() {
    run(vec![
        Case {
            name: "no ceiling anywhere admits any model",
            snapshot: common::empty_snapshot(),
            request: model_use(chain_of_a(), chain_of_a(), "any-model", Phase::ProtectedAction),
            expected: None,
        },
        Case {
            name: "a ceiling elsewhere is no ceiling here",
            snapshot: common::builder()
                .model_ceiling(node("elsewhere"), 2, model_ceiling(&["other"]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "any-model", Phase::ProtectedAction),
            expected: None,
        },
        Case {
            name: "an egress permit does not stand in for anything on a model",
            snapshot: common::builder()
                .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "any-model", Phase::ProtectedAction),
            expected: None,
        },
    ]);
}

#[test]
fn model_ceilings_on_the_lineage_all_have_to_admit() {
    let excluded = |subject, revision| {
        Some(Refusal::CeilingExcludes(rule_ref(
            RuleKind::ModelCeiling,
            subject,
            revision,
        )))
    };
    run(vec![
        Case {
            name: "named by the ceiling on the agent",
            snapshot: common::builder()
                .model_ceiling(agent("a"), 1, model_ceiling(&["m", "n"]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction),
            expected: None,
        },
        Case {
            name: "excluded by the ceiling on the agent",
            snapshot: common::builder()
                .model_ceiling(agent("a"), 7, model_ceiling(&["n"]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction),
            expected: excluded(agent("a"), 7),
        },
        Case {
            name: "excluded by the ceiling on an ancestor node",
            snapshot: common::builder()
                .model_ceiling(node("org"), 5, model_ceiling(&["n"]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction),
            expected: excluded(node("org"), 5),
        },
        Case {
            name: "a ceiling that exists and names nothing admits nothing",
            snapshot: common::builder()
                .model_ceiling(node("team"), 2, model_ceiling(&[]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction),
            expected: excluded(node("team"), 2),
        },
        Case {
            name: "the narrower of two ceilings wins: admitted by both",
            snapshot: common::builder()
                .model_ceiling(node("team"), 1, model_ceiling(&["m", "n"]))
                .model_ceiling(node("org"), 1, model_ceiling(&["n", "o"]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "n", Phase::ProtectedAction),
            expected: None,
        },
        Case {
            name: "the narrower of two ceilings wins: excluded by the farther one",
            snapshot: common::builder()
                .model_ceiling(node("team"), 1, model_ceiling(&["m", "n"]))
                .model_ceiling(node("org"), 9, model_ceiling(&["n", "o"]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction),
            expected: excluded(node("org"), 9),
        },
        Case {
            name: "a ceiling on a delegator above the callee applies to the callee's request",
            snapshot: common::builder()
                .model_ceiling(agent("root"), 3, model_ceiling(&["n"]))
                .build(),
            request: model_use(
                chain_of_a(),
                chain(vec![agent("root"), agent("a"), node("team"), node("org")]),
                "m",
                Phase::ProtectedAction,
            ),
            expected: excluded(agent("root"), 3),
        },
        Case {
            name: "the same ceiling does not bind a request with no such delegator",
            snapshot: common::builder()
                .model_ceiling(agent("root"), 3, model_ceiling(&["n"]))
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction),
            expected: None,
        },
    ]);
}

#[test]
fn holds_refuse_by_scope_and_phase() {
    let permit = |builder: trogon_policy_eval::PolicySnapshotBuilder| {
        builder.egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
    };
    run(vec![
        Case {
            name: "no hold",
            snapshot: permit(common::builder()).build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: None,
        },
        Case {
            name: "NEW_WORK lets a running session finish",
            snapshot: permit(common::builder())
                .holds(agent("a"), 2, [hold("freeze", HoldScope::NewWork)])
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: None,
        },
        Case {
            name: "NEW_WORK refuses admission of a session",
            snapshot: permit(common::builder())
                .holds(agent("a"), 2, [hold("freeze", HoldScope::NewWork)])
                .build(),
            request: Request::without_delegation(
                chain_of_a(),
                trogon_policy_eval::Action::EgressConnect {
                    host: common::host("example.com"),
                    port: common::port(443),
                },
                Phase::SessionAdmission,
            ),
            expected: Some(hold_placed(agent("a"), 2, "freeze")),
        },
        Case {
            name: "ALL_PROTECTED_ACTIONS stops a running session",
            snapshot: permit(common::builder())
                .holds(agent("a"), 2, [hold("stop", HoldScope::AllProtectedActions)])
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: Some(hold_placed(agent("a"), 2, "stop")),
        },
        Case {
            name: "a hold on an ancestor node reaches the agent",
            snapshot: permit(common::builder())
                .holds(node("org"), 11, [hold("org-wide", HoldScope::AllProtectedActions)])
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: Some(hold_placed(node("org"), 11, "org-wide")),
        },
        Case {
            name: "a hold on a delegator reaches the callee's session",
            snapshot: permit(common::builder())
                .holds(agent("root"), 4, [hold("root-stop", HoldScope::AllProtectedActions)])
                .build(),
            request: Request::new(
                chain_of_a(),
                chain(vec![agent("root"), agent("a"), node("team"), node("org")]),
                trogon_policy_eval::Action::EgressConnect {
                    host: common::host("example.com"),
                    port: common::port(443),
                },
                Phase::ProtectedAction,
            )
            .expect("lineage holds the own chain"),
            expected: Some(hold_placed(agent("root"), 4, "root-stop")),
        },
        Case {
            name: "a hold on an unrelated subject does nothing",
            snapshot: permit(common::builder())
                .holds(agent("z"), 2, [hold("stop", HoldScope::AllProtectedActions)])
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: None,
        },
        Case {
            name: "a released hold leaves an empty history that refuses nothing",
            snapshot: permit(common::builder()).holds(agent("a"), 9, []).build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: None,
        },
        Case {
            name: "a hold needs no permit to refuse, and the missing permit is reported first",
            snapshot: common::builder()
                .holds(agent("a"), 2, [hold("stop", HoldScope::AllProtectedActions)])
                .build(),
            request: egress(chain_of_a(), "example.com", 443),
            expected: Some(Refusal::NoPermit(RuleKind::EgressPermission)),
        },
        Case {
            name: "a hold refuses a model request that has no ceiling",
            snapshot: common::builder()
                .holds(node("team"), 2, [hold("stop", HoldScope::AllProtectedActions)])
                .build(),
            request: model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction),
            expected: Some(hold_placed(node("team"), 2, "stop")),
        },
    ]);
}

#[test]
fn delegation_needs_a_permit_that_covers_the_resolved_callee() {
    let no_permit = Some(Refusal::NoPermit(RuleKind::DelegationPermission));
    run(vec![
        Case {
            name: "no delegation permit",
            snapshot: common::empty_snapshot(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: no_permit.clone(),
        },
        Case {
            name: "permit naming the callee",
            snapshot: common::builder()
                .delegation_permission(
                    agent("a"),
                    1,
                    delegation_permission(vec![DelegationTarget::Agent(agent_id("b"))]),
                )
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: None,
        },
        Case {
            name: "permit naming a different callee",
            snapshot: common::builder()
                .delegation_permission(
                    agent("a"),
                    1,
                    delegation_permission(vec![DelegationTarget::Agent(agent_id("c"))]),
                )
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: no_permit.clone(),
        },
        Case {
            name: "permit naming a node the callee sits under",
            snapshot: common::builder()
                .delegation_permission(
                    node("team"),
                    1,
                    delegation_permission(vec![DelegationTarget::AgentsUnder(node_id("callee-team"))]),
                )
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: None,
        },
        Case {
            name: "permit naming a node the callee does not sit under",
            snapshot: common::builder()
                .delegation_permission(
                    node("team"),
                    1,
                    delegation_permission(vec![DelegationTarget::AgentsUnder(node_id("other-team"))]),
                )
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: no_permit.clone(),
        },
        Case {
            name: "an egress permit is not a delegation permit",
            snapshot: common::builder()
                .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: no_permit,
        },
    ]);
}

#[test]
fn a_hold_on_the_callee_refuses_a_delegation_in_either_phase() {
    let permit = || {
        common::builder().delegation_permission(
            agent("a"),
            1,
            delegation_permission(vec![DelegationTarget::AgentsUnder(node_id("callee-team"))]),
        )
    };
    run(vec![
        Case {
            name: "NEW_WORK on the callee agent while the dispatcher is running",
            snapshot: permit()
                .holds(agent("b"), 6, [hold("no-new-work", HoldScope::NewWork)])
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: Some(hold_placed(agent("b"), 6, "no-new-work")),
        },
        Case {
            name: "NEW_WORK on a node above the callee",
            snapshot: permit()
                .holds(node("callee-team"), 8, [hold("no-new-work", HoldScope::NewWork)])
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: Some(hold_placed(node("callee-team"), 8, "no-new-work")),
        },
        Case {
            name: "ALL_PROTECTED_ACTIONS on the callee agent",
            snapshot: permit()
                .holds(agent("b"), 6, [hold("stop", HoldScope::AllProtectedActions)])
                .build(),
            request: a_delegating_to_b(Phase::SessionAdmission),
            expected: Some(hold_placed(agent("b"), 6, "stop")),
        },
        Case {
            name: "NEW_WORK on the dispatcher does not stop a running session delegating",
            snapshot: permit()
                .holds(agent("a"), 6, [hold("no-new-work", HoldScope::NewWork)])
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: None,
        },
        Case {
            name: "a hold on an unrelated callee does nothing",
            snapshot: permit()
                .holds(agent("c"), 6, [hold("stop", HoldScope::AllProtectedActions)])
                .build(),
            request: a_delegating_to_b(Phase::ProtectedAction),
            expected: None,
        },
    ]);
}

#[test]
fn absent_histories_are_consulted_at_revision_zero() {
    let snapshot = common::builder()
        .egress_permission(node("team"), 3, egress_permission(&[("example.com", 443)]))
        .build();
    let decision = enforce().evaluate(
        &view(snapshot),
        &egress(chain_of_a(), "example.com", 443),
        &[],
        fresh_now(),
    );

    assert_eq!(decision.would_deny(), None);
    let consulted = decision.consulted();
    let revision_of = |kind, subject| {
        consulted
            .iter()
            .find(|rule| rule.kind() == kind && rule.subject() == &subject)
            .map(|rule| rule.revision().get())
    };
    assert_eq!(revision_of(RuleKind::EgressPermission, agent("a")), Some(0));
    assert_eq!(revision_of(RuleKind::EgressPermission, node("team")), Some(3));
    assert_eq!(revision_of(RuleKind::EgressPermission, node("org")), Some(0));
    assert_eq!(revision_of(RuleKind::Hold, agent("a")), Some(0));
    assert_eq!(revision_of(RuleKind::Hold, node("org")), Some(0));
    assert_eq!(revision_of(RuleKind::ModelCeiling, agent("a")), None);
}
