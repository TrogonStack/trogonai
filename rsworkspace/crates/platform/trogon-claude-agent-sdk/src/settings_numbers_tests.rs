#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;

use super::{finite_number, normalize_numbers};
use crate::settings_dialect::SettingsDialect;

fn printed(text: &str, dialect: SettingsDialect) -> String {
    let mut value: Value = serde_json::from_str(text).expect("json");
    normalize_numbers(&mut value, dialect);
    value.to_string()
}

fn typescript(text: &str) -> String {
    printed(text, SettingsDialect::TypeScript)
}

#[test]
fn integers_above_two_to_the_fifty_third_print_like_javascript() {
    assert_eq!(typescript("12345678901234567890"), "12345678901234567000");
    assert_eq!(typescript("9007199254740993"), "9007199254740992");
    assert_eq!(typescript("9007199254740995"), "9007199254740996");
    assert_eq!(typescript("-9007199254740993"), "-9007199254740992");
    assert_eq!(typescript("18446744073709549568"), "18446744073709550000");
    assert_eq!(typescript("9223372036854775807"), "9223372036854776000");
    assert_eq!(typescript("-9223372036854774784"), "-9223372036854775000");
}

#[test]
fn whole_doubles_print_as_integers() {
    assert_eq!(typescript("1.0"), "1");
    assert_eq!(typescript("-1.0"), "-1");
    assert_eq!(typescript("1e2"), "100");
    assert_eq!(typescript("1.2345678901234567e19"), "12345678901234567000");
    assert_eq!(typescript("1e16"), "10000000000000000");
}

#[test]
fn negative_zero_prints_as_zero() {
    assert_eq!(typescript("-0"), "0");
    assert_eq!(typescript("-0.0"), "0");
    assert_eq!(typescript("0e5"), "0");
}

#[test]
fn fractions_and_exact_integers_stay() {
    assert_eq!(typescript("0.1"), "0.1");
    assert_eq!(typescript("-1.5"), "-1.5");
    assert_eq!(typescript("9007199254740991"), "9007199254740991");
    assert_eq!(typescript("9007199254740992"), "9007199254740992");
    assert_eq!(typescript("1e21"), "1e+21");
    assert_eq!(printed("1e21", SettingsDialect::Python), "1e+21");
}

#[test]
fn numbers_inside_containers_are_normalized() {
    assert_eq!(
        typescript(r#"{"a":[12345678901234567890,{"b":2.0}],"c":"12345678901234567890"}"#),
        r#"{"a":[12345678901234567000,{"b":2}],"c":"12345678901234567890"}"#
    );
}

#[test]
fn the_python_dialect_keeps_integer_tokens_exact() {
    let python = |text: &str| printed(text, SettingsDialect::Python);
    assert_eq!(python("12345678901234567890"), "12345678901234567890");
    assert_eq!(python("9007199254740993"), "9007199254740993");
    assert_eq!(python("-9223372036854775808"), "-9223372036854775808");
    assert_eq!(python("1.0"), "1");
    assert_eq!(python("-0"), "0");
}

#[test]
fn normalizing_twice_changes_nothing() {
    for text in ["12345678901234567890", "-0", "1.0", "0.1", "1e21", "9007199254740993"] {
        let once = typescript(text);
        assert_eq!(typescript(&once), once);
    }
}

#[test]
fn a_finite_double_never_reads_back_as_an_infinity() {
    assert_eq!(
        finite_number(9_223_372_036_854_775_808.0),
        Value::from(9_223_372_036_854_775_808_u64)
    );
    assert_eq!(
        finite_number(-4_611_686_018_427_387_904.0),
        Value::from(-4_611_686_018_427_387_904_i64)
    );
    assert_eq!(finite_number(1.5), Value::from(1.5));
    assert_eq!(finite_number(f64::NAN), Value::Null);
}
