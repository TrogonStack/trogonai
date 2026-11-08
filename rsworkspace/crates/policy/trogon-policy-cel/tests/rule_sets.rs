#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use common::{engine, gateway_facts, limits, refusing_rule, rule_name};
use trogon_policy_cel::constants::MAX_RULES_PER_SET;
use trogon_policy_cel::{BundleHandle, CelEngine, CelVerdict, CompileError, RuleSet, RuleSetError};
use trogon_policy_eval::{Narrowing, RequestRuleEngine};

#[test]
fn an_empty_rule_set_passes_every_request() {
    let rules = RuleSet::empty();
    assert!(rules.is_empty());
    assert_eq!(rules.len(), 0);
    let engine = CelEngine::new(limits(), BundleHandle::new(rules));
    assert_eq!(engine.evaluate(&gateway_facts()), CelVerdict::Pass);
    assert_eq!(
        engine.evaluate(&trogon_policy_cel::RequestFacts::new()),
        CelVerdict::Pass
    );
    assert_eq!(engine.narrow(&gateway_facts()), Narrowing::Pass);
}

#[test]
fn every_rule_must_pass() {
    let table = [
        (vec![("a", "true"), ("b", "true")], CelVerdict::Pass),
        (vec![("a", "true")], CelVerdict::Pass),
    ];
    for (rules, expected) in table {
        assert_eq!(engine(&rules).evaluate(&gateway_facts()), expected);
    }
    let verdict = engine(&[("a", "true"), ("b", "false"), ("c", "true")]).evaluate(&gateway_facts());
    assert_eq!(refusing_rule(&verdict), "b");
}

#[test]
fn the_refusing_rule_is_the_first_by_name_whatever_order_they_were_supplied_in() {
    let orders: [[(&str, &str); 3]; 3] = [
        [("zeta", "false"), ("alpha", "false"), ("mid", "true")],
        [("mid", "true"), ("alpha", "false"), ("zeta", "false")],
        [("alpha", "false"), ("zeta", "false"), ("mid", "true")],
    ];
    for order in orders {
        let verdict = engine(&order).evaluate(&gateway_facts());
        assert_eq!(refusing_rule(&verdict), "alpha");
    }
}

#[test]
fn a_rule_that_cannot_answer_refuses_ahead_of_a_later_rule_that_is_false() {
    let verdict = engine(&[("a", "missing_variable"), ("b", "false")]).evaluate(&gateway_facts());
    assert_eq!(refusing_rule(&verdict), "a");
}

#[test]
fn the_narrowing_names_exactly_the_refusing_rule() {
    let engine = engine(&[("keep", "true"), ("no-exfil", r#"request.host != "api.example.com""#)]);
    assert_eq!(engine.narrow(&gateway_facts()), Narrowing::Deny(rule_name("no-exfil")));
}

#[test]
fn a_set_loads_whole_or_not_at_all() {
    let err = RuleSet::compile([(rule_name("good"), "true"), (rule_name("bad"), "1 +")], &limits()).expect_err("fails");
    match err {
        RuleSetError::Rule { name, source } => {
            assert_eq!(name, rule_name("bad"));
            assert!(matches!(source, CompileError::Parse { .. }));
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn two_rules_with_one_name_are_refused() {
    let err =
        RuleSet::compile([(rule_name("same"), "true"), (rule_name("same"), "false")], &limits()).expect_err("dup");
    assert_eq!(
        err,
        RuleSetError::Duplicate {
            name: rule_name("same")
        }
    );
}

#[test]
fn a_set_larger_than_the_limit_is_refused() {
    let sources = (0..=MAX_RULES_PER_SET).map(|i| (rule_name(&format!("rule-{i:04}")), "true"));
    let err = RuleSet::compile(sources, &limits()).expect_err("too many");
    assert_eq!(
        err,
        RuleSetError::TooMany {
            actual: MAX_RULES_PER_SET + 1,
            limit: MAX_RULES_PER_SET
        }
    );
    let at_limit = (0..MAX_RULES_PER_SET).map(|i| (rule_name(&format!("rule-{i:04}")), "true"));
    assert_eq!(
        RuleSet::compile(at_limit, &limits()).expect("fits").len(),
        MAX_RULES_PER_SET
    );
}

#[test]
fn the_rules_are_kept_in_name_order() {
    let rules = RuleSet::compile(
        [
            (rule_name("b"), "true"),
            (rule_name("c"), "true"),
            (rule_name("a"), "true"),
        ],
        &limits(),
    )
    .expect("compiles");
    let names: Vec<&str> = rules.rules().iter().map(|r| r.name().as_str()).collect();
    assert_eq!(names, ["a", "b", "c"]);
}
