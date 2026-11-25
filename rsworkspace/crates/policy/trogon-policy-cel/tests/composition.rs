//! The evaluator and the CEL engine together: bindings admission AND every
//! request rule, with the rules unable to widen anything.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::cell::Cell;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{engine, gateway_facts};
use trogon_policy_cel::{CelEngine, RequestFacts};
use trogon_policy_eval::{
    Action, AgentId, Chain, Effective, EgressDestination, EgressHostName, EgressPermission, EnforcementMode, Evaluator,
    InMemoryPolicyView, Narrowing, NodeId, Phase, PolicySnapshot, Port, Refusal, Request, RequestRuleEngine, RuleKind,
    StalenessBudget, Subject,
};

fn stamped_at() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000)
}

fn now() -> SystemTime {
    stamped_at() + Duration::from_secs(1)
}

fn agent() -> Subject {
    Subject::Agent(AgentId::new("a").expect("valid"))
}

fn request() -> Request {
    let chain = Chain::new(vec![agent(), Subject::Node(NodeId::new("org").expect("valid"))]).expect("non-empty");
    Request::without_delegation(
        chain,
        Action::EgressConnect {
            host: EgressHostName::new("api.example.com").expect("valid"),
            port: Port::new(443).expect("valid"),
        },
        Phase::ProtectedAction,
    )
}

fn permitted_view() -> InMemoryPolicyView {
    let destination = EgressDestination::new(
        EgressHostName::new("api.example.com").expect("valid"),
        Port::new(443).expect("valid"),
    );
    let permission: EgressPermission = [destination].into_iter().collect();
    InMemoryPolicyView::new(
        PolicySnapshot::builder(stamped_at())
            .egress_permission(agent(), 1, permission)
            .build(),
    )
}

fn unpermitted_view() -> InMemoryPolicyView {
    InMemoryPolicyView::new(PolicySnapshot::builder(stamped_at()).build())
}

fn evaluator() -> Evaluator {
    Evaluator::new(EnforcementMode::Enforce, StalenessBudget::default())
}

fn run(view: &InMemoryPolicyView, engine: &CelEngine) -> Effective {
    evaluator()
        .evaluate_with(view, &request(), engine, &gateway_facts(), now())
        .effective()
        .clone()
}

#[test]
fn admitted_when_the_bindings_admit_and_every_rule_passes() {
    let engine = engine(&[("a", "true"), ("b", r#"request.method == "POST""#)]);
    assert_eq!(run(&permitted_view(), &engine), Effective::Admit);
}

#[test]
fn refused_by_the_rule_that_refuses_when_the_bindings_admit() {
    let engine = engine(&[("a", "true"), ("no-get", r#"request.method == "GET""#)]);
    let effective = run(&permitted_view(), &engine);
    match effective {
        Effective::Refuse(Refusal::RequestRuleDenied(rule)) => assert_eq!(rule.as_str(), "no-get"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_rule_that_cannot_answer_refuses_the_request() {
    let engine = engine(&[("broken", "request.nope == 1")]);
    assert!(matches!(
        run(&permitted_view(), &engine),
        Effective::Refuse(Refusal::RequestRuleDenied(_))
    ));
}

#[test]
fn rules_that_pass_never_stand_in_for_a_missing_permit() {
    let engine = engine(&[("always", "true")]);
    assert_eq!(
        run(&unpermitted_view(), &engine),
        Effective::Refuse(Refusal::NoPermit(RuleKind::EgressPermission))
    );
}

#[test]
fn the_bindings_refusal_is_the_one_named_when_both_would_refuse() {
    let engine = engine(&[("never", "false")]);
    assert_eq!(
        run(&unpermitted_view(), &engine),
        Effective::Refuse(Refusal::NoPermit(RuleKind::EgressPermission))
    );
}

#[test]
fn an_empty_rule_set_leaves_the_decision_to_the_bindings() {
    let engine = engine(&[]);
    assert_eq!(run(&permitted_view(), &engine), Effective::Admit);
    assert_eq!(
        run(&unpermitted_view(), &engine),
        Effective::Refuse(Refusal::NoPermit(RuleKind::EgressPermission))
    );
}

/// Counts how often it is asked, and refuses every time.
struct CountingEngine {
    calls: Cell<u32>,
}

impl RequestRuleEngine for CountingEngine {
    type Facts = RequestFacts;

    fn narrow(&self, _facts: &RequestFacts) -> Narrowing {
        self.calls.set(self.calls.get() + 1);
        Narrowing::Deny(common::rule_name("counted"))
    }
}

#[test]
fn the_engine_is_consulted_only_when_the_bindings_admit() {
    let engine = CountingEngine { calls: Cell::new(0) };
    let facts = RequestFacts::new();

    let refused = evaluator().evaluate_with(&unpermitted_view(), &request(), &engine, &facts, now());
    assert_eq!(
        refused.effective(),
        &Effective::Refuse(Refusal::NoPermit(RuleKind::EgressPermission))
    );
    assert_eq!(engine.calls.get(), 0);

    let denied = evaluator().evaluate_with(&permitted_view(), &request(), &engine, &facts, now());
    assert!(matches!(
        denied.effective(),
        Effective::Refuse(Refusal::RequestRuleDenied(_))
    ));
    assert_eq!(engine.calls.get(), 1);
}
