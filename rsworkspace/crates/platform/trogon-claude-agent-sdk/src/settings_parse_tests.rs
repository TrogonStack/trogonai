#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;

use super::parse_json;
use crate::settings_numbers::{infinities_to_null, js_value};

fn infinities(value: &Value) -> Vec<f64> {
    match value {
        Value::Number(number) => js_value(number)
            .filter(|number| number.is_infinite())
            .into_iter()
            .collect(),
        Value::Array(items) => items.iter().flat_map(infinities).collect(),
        Value::Object(entries) => entries.values().flat_map(infinities).collect(),
        _ => Vec::new(),
    }
}

#[test]
fn a_document_without_overflowing_numbers_reads_as_serde_json_reads_it() {
    let text = r#"{"a":[1,-0,1.5e300,12345678901234567890,"1e400"],"b":{"c":null,"d":true}}"#;
    assert_eq!(parse_json(text).unwrap(), serde_json::from_str::<Value>(text).unwrap());
}

#[test]
fn numbers_past_the_double_range_read_as_infinities() {
    let value = parse_json(r#"{"a":1e400,"b":[-1e400,2,1.7976931348623159e308],"c":{"d":-1e309}}"#).unwrap();
    assert_eq!(
        infinities(&value),
        vec![f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY]
    );
    assert_eq!(value["b"][1], Value::from(2));
    let mut printed = value;
    infinities_to_null(&mut printed);
    assert_eq!(printed.to_string(), r#"{"a":null,"b":[null,2,null],"c":{"d":null}}"#);
}

#[test]
fn digits_inside_strings_are_not_number_tokens() {
    let value = parse_json(r#"{"1e400":"-1e400 \" 5","k\\":[1e400,"\\",3]}"#).unwrap();
    assert_eq!(value["1e400"], Value::from("-1e400 \" 5"));
    assert_eq!(value["k\\"][2], Value::from(3));
    assert_eq!(infinities(&value), vec![f64::INFINITY]);
}

#[test]
fn a_repeated_key_keeps_its_first_position_and_its_last_value() {
    let mut value = parse_json(r#"{"a":1e400,"b":1,"a":-1e400,"c":1e400}"#).unwrap();
    assert_eq!(infinities(&value), vec![f64::NEG_INFINITY, f64::INFINITY]);
    infinities_to_null(&mut value);
    assert_eq!(value.to_string(), r#"{"a":null,"b":1,"c":null}"#);
}

#[test]
fn invalid_text_is_still_an_error() {
    assert!(parse_json("{\"a\":1e400").is_err());
    assert!(parse_json("{\"a\":1e400} x").is_err());
    assert!(parse_json("[01e400]").is_err());
    assert!(parse_json("[1e400.5]").is_err());
    assert!(parse_json("[-1e400e1]").is_err());
}

#[test]
fn a_finite_number_with_the_value_of_an_infinity_marker_keeps_its_value() {
    let value = parse_json("[9223372036854775808.0,-4611686018427387904.0,1e400]").unwrap();
    assert_eq!(infinities(&value), vec![f64::INFINITY]);
    assert_eq!(value[0], Value::from(9_223_372_036_854_775_808_u64));
    assert_eq!(value[1], Value::from(-4_611_686_018_427_387_904_i64));
    let value = parse_json("[9.223372036854775808e18]").unwrap();
    assert!(infinities(&value).is_empty());
    assert_eq!(value[0], Value::from(9_223_372_036_854_775_808_u64));
}
