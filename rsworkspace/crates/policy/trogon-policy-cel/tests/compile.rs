#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{compile, limits, rule_name};
use trogon_policy_cel::constants::{MAX_EXPRESSION_DEPTH, MAX_SOURCE_BYTES};
use trogon_policy_cel::{CelLimits, CompileError, CompiledRule};

#[test]
fn rules_that_could_never_run_well_are_refused_at_compile_time() {
    let deep_brackets = format!(
        "{}1{}",
        "(".repeat(MAX_EXPRESSION_DEPTH + 1),
        ")".repeat(MAX_EXPRESSION_DEPTH + 1)
    );
    let prefix_run = format!("{}true", "!".repeat(MAX_EXPRESSION_DEPTH + 1));
    let conditionals = format!("true{}", " ? true : false".repeat(MAX_EXPRESSION_DEPTH + 1));
    let too_long = format!("true{}", " ".repeat(MAX_SOURCE_BYTES));
    let table: Vec<(&str, String, CompileError)> = vec![
        ("empty", String::new(), CompileError::Empty),
        ("blank", " \n\t ".to_owned(), CompileError::Empty),
        (
            "too long",
            too_long.clone(),
            CompileError::SourceTooLong {
                actual: too_long.len(),
                limit: MAX_SOURCE_BYTES,
            },
        ),
        (
            "brackets nested too deep",
            deep_brackets,
            CompileError::TooDeep {
                limit: MAX_EXPRESSION_DEPTH,
            },
        ),
        (
            "prefix operators chained too far",
            prefix_run,
            CompileError::TooDeep {
                limit: MAX_EXPRESSION_DEPTH,
            },
        ),
        (
            "conditionals chained too far",
            conditionals,
            CompileError::TooDeep {
                limit: MAX_EXPRESSION_DEPTH,
            },
        ),
        (
            "a struct literal",
            "Thing{a: 1}.a == 1".to_owned(),
            CompileError::Unsupported {
                construct: "a struct literal",
            },
        ),
    ];
    for (label, source, expected) in table {
        assert_eq!(compile(&source).expect_err(label), expected, "{label}");
    }
}

#[test]
fn a_source_that_is_not_an_expression_is_a_parse_error() {
    for source in [
        "request.method ==",
        "1 +* 2",
        "(true",
        "true )",
        "a b",
        "\"unterminated",
    ] {
        assert!(
            matches!(compile(source), Err(CompileError::Parse { .. })),
            "{source:?} should not parse"
        );
    }
}

#[test]
fn nesting_just_inside_the_limit_still_compiles() {
    let source = format!(
        "{}true{}",
        "(".repeat(MAX_EXPRESSION_DEPTH - 2),
        ")".repeat(MAX_EXPRESSION_DEPTH - 2)
    );
    assert!(compile(&source).is_ok());
}

#[test]
fn a_long_chain_of_operators_is_refused_for_its_depth_without_overflowing_the_stack() {
    let sum = vec!["1"; 1_500].join(" + ");
    let source = format!("{sum} > 0");
    assert!(source.len() < MAX_SOURCE_BYTES);
    assert_eq!(
        compile(&source).expect_err("too deep"),
        CompileError::TooDeep {
            limit: MAX_EXPRESSION_DEPTH
        }
    );
}

#[test]
fn cost_is_a_static_upper_bound_with_known_weights() {
    // literal 1; identifier 1; selection 1 + operand; call 2 + operands.
    let table = [
        ("true", 1),
        ("request.port", 2),
        ("request.port == 443", 2 + 2 + 1),
        // A map literal is a node over its keys and values, then selected.
        (r#"{"a": 1}.a == 1"#, 2 + (1 + (1 + (1 + 1))) + 1),
        ("request.port == 443 && caller.depth < 3", 2 + (2 + 2 + 1) + (2 + 2 + 1)),
        (r#"request.host.startsWith("api.")"#, 8 + 2 + 1),
        (r#"request.host.matches("^api")"#, 64 + 2 + 1),
        // The branch that costs more is the one counted.
        ("true ? 1 == 1 : 1", 2 + 1 + (2 + 1 + 1)),
    ];
    for (source, expected) in table {
        let rule = compile(source).expect("compiles");
        assert_eq!(rule.cost(), expected, "{source}");
    }
}

#[test]
fn a_loop_costs_its_body_once_per_possible_element() {
    let cheap = compile("request.tags.all(t, t != \"x\")").expect("compiles");
    let defaults = limits();
    assert!(cheap.cost() > defaults.max_collection_len() as u64);
    assert!(cheap.cost() < defaults.max_cost());
}

#[test]
fn nested_loops_multiply_and_are_refused_when_they_exceed_the_cost_limit() {
    let source = "a.all(x, b.all(y, c.all(z, x == y && y == z)))";
    assert!(matches!(compile(source), Err(CompileError::CostExceeded { .. })));
}

#[test]
fn the_cost_limit_is_the_callers_to_set() {
    let source = r#"request.host.matches("^api")"#;
    let tight = CelLimits::new(10, Duration::from_secs(1)).expect("non-zero limits");
    assert!(matches!(
        CompiledRule::compile(rule_name("r"), source, &tight),
        Err(CompileError::CostExceeded { cost: 67, limit: 10 })
    ));
    let loose = CelLimits::new(67, Duration::from_secs(1)).expect("non-zero limits");
    assert!(CompiledRule::compile(rule_name("r"), source, &loose).is_ok());
}

#[test]
fn a_shorter_collection_limit_lowers_the_cost_of_loops() {
    let source = "request.tags.all(t, t != \"x\")";
    let wide = CelLimits::default();
    let narrow = CelLimits::default().with_max_collection_len(4).expect("non-zero");
    let wide_cost = CompiledRule::compile(rule_name("r"), source, &wide)
        .expect("compiles")
        .cost();
    let narrow_cost = CompiledRule::compile(rule_name("r"), source, &narrow)
        .expect("compiles")
        .cost();
    assert!(narrow_cost < wide_cost);
}

#[test]
fn a_literal_longer_than_the_collection_limit_still_counts_at_its_length() {
    let items = vec!["1"; 300].join(", ");
    let source = format!("[{items}].all(n, n > 0)");
    let tight = CelLimits::new(1_000, Duration::from_secs(1)).expect("non-zero limits");
    assert!(matches!(
        CompiledRule::compile(rule_name("r"), &source, &tight),
        Err(CompileError::CostExceeded { .. })
    ));
}

#[test]
fn zero_limits_are_refused_where_they_are_set() {
    assert!(CelLimits::new(0, Duration::from_secs(1)).is_err());
    assert!(CelLimits::new(1, Duration::ZERO).is_err());
    assert!(CelLimits::default().with_max_input_bytes(0).is_err());
    assert!(CelLimits::default().with_max_collection_len(0).is_err());
}
