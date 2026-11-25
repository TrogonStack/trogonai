#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use common::{cause_of, fact_name};
use trogon_policy_cel::constants::{MAX_FACT_NAME_BYTES, MAX_FACT_NESTING};
use trogon_policy_cel::{
    BundleHandle, CelEngine, CelLimits, CelVerdict, FactName, FactNameError, FactValue, FactsLimitError, RefusalCause,
    RequestFacts, RuleSet,
};

#[test]
fn a_fact_name_must_be_usable_as_an_identifier() {
    let long = "a".repeat(MAX_FACT_NAME_BYTES + 1);
    let table: Vec<(&str, FactNameError)> = vec![
        ("", FactNameError::Empty),
        (
            long.as_str(),
            FactNameError::TooLong {
                actual: MAX_FACT_NAME_BYTES + 1,
                limit: MAX_FACT_NAME_BYTES,
            },
        ),
        ("9lives", FactNameError::StartsWithDigit),
        ("has space", FactNameError::InvalidCharacter { character: ' ' }),
        ("dotted.name", FactNameError::InvalidCharacter { character: '.' }),
        ("dash-ed", FactNameError::InvalidCharacter { character: '-' }),
        ("caf\u{e9}", FactNameError::InvalidCharacter { character: '\u{e9}' }),
        ("true", FactNameError::Reserved),
        ("null", FactNameError::Reserved),
        ("in", FactNameError::Reserved),
    ];
    for (input, expected) in table {
        assert_eq!(FactName::new(input).expect_err(input), expected, "{input:?}");
    }
    for valid in ["request", "_private", "caller2", "A", "a_b_c"] {
        assert_eq!(FactName::new(valid).expect("valid").as_str(), valid);
    }
}

#[test]
fn values_convert_from_the_types_a_host_has() {
    assert_eq!(FactValue::from(true), FactValue::Bool(true));
    assert_eq!(FactValue::from(7_i64), FactValue::Int(7));
    assert_eq!(FactValue::from(7_i32), FactValue::Int(7));
    assert_eq!(FactValue::from(443_u16), FactValue::Int(443));
    assert_eq!(FactValue::from("x"), FactValue::String("x".to_owned()));
    assert_eq!(FactValue::from(None::<&str>), FactValue::Null);
    assert_eq!(FactValue::from(Some("x")), FactValue::String("x".to_owned()));
    assert_eq!(
        FactValue::from(vec!["a", "b"]),
        FactValue::List(vec![FactValue::from("a"), FactValue::from("b")])
    );
}

#[test]
fn later_facts_replace_earlier_ones_of_the_same_name() {
    let facts = RequestFacts::new().with(fact_name("x"), 1).with(fact_name("x"), 2);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts.get("x"), Some(&FactValue::Int(2)));
    assert!(RequestFacts::new().is_empty());
}

fn engine_with(limits: CelLimits, source: &str) -> CelEngine {
    let rules = RuleSet::compile([(common::rule_name("r"), source)], &limits).expect("compiles");
    CelEngine::new(
        CelLimits::new(limits.max_cost(), std::time::Duration::from_secs(30))
            .expect("non-zero")
            .with_max_input_bytes(limits.max_input_bytes())
            .expect("non-zero")
            .with_max_collection_len(limits.max_collection_len())
            .expect("non-zero"),
        BundleHandle::new(rules),
    )
}

fn nested(levels: usize) -> FactValue {
    (0..levels).fold(FactValue::Int(1), |inner, _| FactValue::list([inner]))
}

#[test]
fn facts_beyond_the_input_limits_are_refused_before_any_rule_reads_them() {
    let small = CelLimits::default()
        .with_max_input_bytes(64)
        .expect("non-zero")
        .with_max_collection_len(3)
        .expect("non-zero");
    let engine = engine_with(small, "true");

    let table: Vec<(&str, FactValue, FactsLimitError)> = vec![
        (
            "too many bytes",
            FactValue::from("x".repeat(100)),
            FactsLimitError::TooManyBytes { limit: 64 },
        ),
        (
            "a list that is too long",
            FactValue::list([1, 2, 3, 4]),
            FactsLimitError::CollectionTooLong { actual: 4, limit: 3 },
        ),
        (
            "a map that is too large",
            FactValue::map([("a", 1), ("b", 2), ("c", 3), ("d", 4)]),
            FactsLimitError::CollectionTooLong { actual: 4, limit: 3 },
        ),
    ];
    for (label, value, rejected) in table {
        let facts = RequestFacts::new().with(fact_name("x"), value);
        assert_eq!(
            cause_of(engine.evaluate(&facts)),
            RefusalCause::FactsRejected(rejected),
            "{label}"
        );
    }
}

#[test]
fn facts_nested_beyond_the_limit_are_refused_and_at_the_limit_are_not() {
    let engine = engine_with(CelLimits::default(), "true");
    let too_deep = RequestFacts::new().with(fact_name("x"), nested(MAX_FACT_NESTING));
    assert_eq!(
        cause_of(engine.evaluate(&too_deep)),
        RefusalCause::FactsRejected(FactsLimitError::NestedTooDeep {
            limit: MAX_FACT_NESTING
        })
    );
    let allowed = RequestFacts::new().with(fact_name("x"), nested(MAX_FACT_NESTING - 1));
    assert_eq!(engine.evaluate(&allowed), CelVerdict::Pass);
}

#[test]
fn facts_within_the_input_limits_are_evaluated() {
    let small = CelLimits::default()
        .with_max_input_bytes(256)
        .expect("non-zero")
        .with_max_collection_len(3)
        .expect("non-zero");
    let engine = engine_with(small, "size(x) == 3");
    let facts = RequestFacts::new().with(fact_name("x"), FactValue::list([1, 2, 3]));
    assert_eq!(engine.evaluate(&facts), CelVerdict::Pass);
}

#[test]
fn the_input_limit_counts_names_as_well_as_values() {
    let small = CelLimits::default().with_max_input_bytes(40).expect("non-zero");
    let engine = engine_with(small, "true");
    let facts = RequestFacts::new().with(fact_name(&"n".repeat(60)), 1);
    assert_eq!(
        cause_of(engine.evaluate(&facts)),
        RefusalCause::FactsRejected(FactsLimitError::TooManyBytes { limit: 40 })
    );
}

#[test]
fn an_empty_rule_set_ignores_the_facts_entirely() {
    let small = CelLimits::default().with_max_input_bytes(8).expect("non-zero");
    let engine = CelEngine::new(small, BundleHandle::new(RuleSet::empty()));
    let facts = RequestFacts::new().with(fact_name("x"), FactValue::from("x".repeat(1_000)));
    assert_eq!(engine.evaluate(&facts), CelVerdict::Pass);
}
