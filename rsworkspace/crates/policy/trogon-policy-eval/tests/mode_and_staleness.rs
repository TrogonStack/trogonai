#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    agent, chain_of_a, egress, egress_permission, enforce, fresh_now, hold, model_use, node, stamped_at, view,
};
use trogon_policy_eval::{
    Effective, EnforcementMode, Evaluator, EvaluatorError, HoldScope, InMemoryPolicyView, Narrowing, Phase,
    PolicySnapshot, PolicyView, Refusal, RuleKind, RuleName, StalenessBudget,
};

fn permitted() -> PolicySnapshot {
    common::builder()
        .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
        .build()
}

fn secs(value: u64) -> Duration {
    Duration::from_secs(value)
}

#[test]
fn dry_run_admits_what_enforce_would_refuse_and_records_it() {
    let request = egress(chain_of_a(), "example.com", 443);
    let enforced = enforce().evaluate(&view(common::empty_snapshot()), &request, &[], fresh_now());
    let dry = common::dry_run().evaluate(&view(common::empty_snapshot()), &request, &[], fresh_now());

    let refusal = Refusal::NoPermit(RuleKind::EgressPermission);
    assert_eq!(enforced.effective(), &Effective::Refuse(refusal.clone()));
    assert_eq!(enforced.would_deny(), Some(&refusal));
    assert_eq!(dry.effective(), &Effective::Admit);
    assert_eq!(dry.would_deny(), Some(&refusal));
    assert_eq!(dry.mode(), EnforcementMode::DryRun);
    assert_eq!(enforced.consulted(), dry.consulted());
}

#[test]
fn an_evaluator_reports_the_mode_and_budget_it_was_built_with() {
    let budget = StalenessBudget::new(secs(60)).expect("within the ceiling");
    let evaluator = Evaluator::new(EnforcementMode::DryRun, budget);
    assert_eq!(evaluator.mode(), EnforcementMode::DryRun);
    assert_eq!(evaluator.staleness_budget(), budget);
}

#[test]
fn modes_have_stable_names_for_configuration_and_logs() {
    assert_eq!(EnforcementMode::DryRun.as_str(), "dry_run");
    assert_eq!(EnforcementMode::Enforce.as_str(), "enforce");
    assert_eq!(EnforcementMode::DryRun.to_string(), "dry_run");
    assert_eq!(EnforcementMode::Enforce.to_string(), "enforce");
}

#[test]
fn both_modes_agree_when_nothing_refuses() {
    let request = egress(chain_of_a(), "example.com", 443);
    for evaluator in [enforce(), common::dry_run()] {
        let decision = evaluator.evaluate(&view(permitted()), &request, &[], fresh_now());
        assert_eq!(decision.effective(), &Effective::Admit);
        assert_eq!(decision.would_deny(), None);
    }
}

#[test]
fn dry_run_records_a_request_rule_denial_without_enforcing_it() {
    let request = egress(chain_of_a(), "example.com", 443);
    let denial = Narrowing::Deny(RuleName::new("no-exfil").expect("valid"));
    let decision = common::dry_run().evaluate(&view(permitted()), &request, &[denial], fresh_now());
    assert_eq!(decision.effective(), &Effective::Admit);
    assert!(matches!(decision.would_deny(), Some(Refusal::RequestRuleDenied(_))));
}

#[test]
fn staleness_is_judged_against_the_budget_at_its_edges() {
    let request = egress(chain_of_a(), "example.com", 443);
    let budget = StalenessBudget::new(secs(60)).expect("within the ceiling");
    let evaluator = Evaluator::new(EnforcementMode::Enforce, budget);

    let cases: Vec<(&str, Duration, bool)> = vec![
        ("just stamped", Duration::ZERO, false),
        ("well inside", secs(30), false),
        ("one nanosecond inside", secs(60) - Duration::from_nanos(1), false),
        ("exactly at the budget", secs(60), false),
        ("one nanosecond past", secs(60) + Duration::from_nanos(1), true),
        ("far past", secs(3600), true),
    ];
    for (name, age, stale) in cases {
        let decision = evaluator.evaluate(&view(permitted()), &request, &[], stamped_at() + age);
        match (stale, decision.effective()) {
            (false, Effective::Admit) => {}
            (true, Effective::Refuse(Refusal::StaleSnapshot { age: reported, budget })) => {
                assert_eq!(*reported, age, "case: {name}");
                assert_eq!(*budget, secs(60), "case: {name}");
            }
            (_, other) => panic!("case {name}: unexpected {other:?}"),
        }
    }
}

#[test]
fn the_default_budget_is_the_330_second_ceiling() {
    let request = egress(chain_of_a(), "example.com", 443);
    let at = |age: Duration| enforce().evaluate(&view(permitted()), &request, &[], stamped_at() + age);

    assert_eq!(at(secs(330)).effective(), &Effective::Admit);
    assert!(matches!(
        at(secs(331)).effective(),
        Effective::Refuse(Refusal::StaleSnapshot { .. })
    ));
}

#[test]
fn a_stale_snapshot_refuses_even_what_bindings_would_admit_and_reads_nothing() {
    let request = egress(chain_of_a(), "example.com", 443);
    let decision = enforce().evaluate(&view(permitted()), &request, &[], stamped_at() + secs(1000));
    assert!(matches!(decision.would_deny(), Some(Refusal::StaleSnapshot { .. })));
    assert!(decision.consulted().is_empty());
}

#[test]
fn dry_run_still_reports_staleness() {
    let request = egress(chain_of_a(), "example.com", 443);
    let decision = common::dry_run().evaluate(&view(permitted()), &request, &[], stamped_at() + secs(1000));
    assert_eq!(decision.effective(), &Effective::Admit);
    assert!(matches!(decision.would_deny(), Some(Refusal::StaleSnapshot { .. })));
}

#[test]
fn a_snapshot_stamped_in_the_future_refuses_rather_than_looking_fresh() {
    let request = egress(chain_of_a(), "example.com", 443);
    let decision = enforce().evaluate(&view(permitted()), &request, &[], stamped_at() - secs(5));
    assert_eq!(
        decision.effective(),
        &Effective::Refuse(Refusal::EvaluatorError(EvaluatorError::SnapshotFromTheFuture {
            by: secs(5)
        }))
    );
}

#[test]
fn a_view_that_stops_advancing_as_of_makes_the_evaluator_refuse() {
    let request = egress(chain_of_a(), "example.com", 443);
    let source = InMemoryPolicyView::new(permitted());
    let evaluator = enforce();

    assert_eq!(
        evaluator
            .evaluate(&source, &request, &[], stamped_at() + secs(100))
            .effective(),
        &Effective::Admit
    );
    // The projector heartbeats: same bindings, a later stamp.
    source.replace(
        PolicySnapshot::builder(stamped_at() + secs(400))
            .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
            .build(),
    );
    assert_eq!(
        evaluator
            .evaluate(&source, &request, &[], stamped_at() + secs(500))
            .effective(),
        &Effective::Admit
    );
    // The projector falls behind: nothing replaces the snapshot.
    assert!(matches!(
        evaluator
            .evaluate(&source, &request, &[], stamped_at() + secs(800))
            .effective(),
        Effective::Refuse(Refusal::StaleSnapshot { .. })
    ));
}

#[test]
fn a_released_hold_stops_refusing_once_the_view_swaps() {
    let request = model_use(chain_of_a(), chain_of_a(), "m", Phase::ProtectedAction);
    let source = Arc::new(InMemoryPolicyView::new(
        common::builder()
            .holds(node("org"), 2, [hold("stop", HoldScope::AllProtectedActions)])
            .build(),
    ));
    let evaluator = enforce();
    assert!(matches!(
        evaluator.evaluate(&source, &request, &[], fresh_now()).effective(),
        Effective::Refuse(Refusal::HoldPlaced { .. })
    ));

    source.replace(common::builder().holds(node("org"), 3, []).build());
    assert_eq!(
        evaluator.evaluate(&source, &request, &[], fresh_now()).effective(),
        &Effective::Admit
    );
}

#[test]
fn an_arc_of_a_view_is_a_view() {
    let source: Arc<dyn PolicyView> = Arc::new(InMemoryPolicyView::new(permitted()));
    let request = egress(chain_of_a(), "example.com", 443);
    let decision = enforce().evaluate(&source, &request, &[], fresh_now());
    assert_eq!(decision.effective(), &Effective::Admit);
}
