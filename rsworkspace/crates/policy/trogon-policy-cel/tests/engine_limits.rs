#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{SteppingClock, cause_of, gateway_facts, limits, refusing_rule, rule_set};
use trogon_policy_cel::{BundleHandle, CelEngine, CelLimits, CelVerdict, RefusalCause};

fn timed(timeout: Duration, step: Duration, rules: &[(&str, &str)]) -> CelEngine<SteppingClock> {
    let limits = CelLimits::new(limits().max_cost(), timeout).expect("non-zero");
    CelEngine::with_clock(limits, BundleHandle::new(rule_set(rules)), SteppingClock::new(step))
}

#[test]
fn evaluation_inside_the_timeout_is_decided_by_the_rules() {
    let engine = timed(
        Duration::from_millis(100),
        Duration::from_millis(10),
        &[("a", "true"), ("b", "true")],
    );
    assert_eq!(engine.evaluate(&gateway_facts()), CelVerdict::Pass);
}

#[test]
fn evaluation_past_the_timeout_refuses_even_when_every_rule_passes() {
    // Each rule takes 40 ms of the injected clock against a 100 ms budget, so
    // the third rule is the one that crosses it.
    let engine = timed(
        Duration::from_millis(100),
        Duration::from_millis(40),
        &[("a", "true"), ("b", "true"), ("c", "true"), ("d", "true")],
    );
    let verdict = engine.evaluate(&gateway_facts());
    assert_eq!(refusing_rule(&verdict), "c");
    assert_eq!(
        cause_of(verdict),
        RefusalCause::TimedOut {
            limit: Duration::from_millis(100)
        }
    );
}

#[test]
fn a_single_slow_rule_refuses_and_names_itself() {
    let engine = timed(Duration::from_millis(5), Duration::from_millis(50), &[("only", "true")]);
    let verdict = engine.evaluate(&gateway_facts());
    assert_eq!(refusing_rule(&verdict), "only");
    assert!(matches!(cause_of(verdict), RefusalCause::TimedOut { .. }));
}

#[test]
fn the_timeout_is_reached_before_a_later_false_rule_can_be_named() {
    let engine = timed(
        Duration::from_millis(30),
        Duration::from_millis(40),
        &[("a", "true"), ("b", "false")],
    );
    let verdict = engine.evaluate(&gateway_facts());
    assert_eq!(refusing_rule(&verdict), "a");
}

#[test]
fn a_rule_that_is_false_is_reported_as_false_when_there_is_time() {
    let engine = timed(Duration::from_secs(1), Duration::from_millis(1), &[("a", "false")]);
    assert_eq!(cause_of(engine.evaluate(&gateway_facts())), RefusalCause::False);
}

#[test]
fn the_engine_reports_the_limits_it_enforces() {
    let engine = timed(Duration::from_millis(7), Duration::ZERO, &[("a", "true")]);
    assert_eq!(engine.limits().timeout(), Duration::from_millis(7));
}
