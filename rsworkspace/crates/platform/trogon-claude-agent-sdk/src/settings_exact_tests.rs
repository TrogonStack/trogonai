#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Map, Value, json};

use super::{js_exact_value, js_stringify_exact};
use crate::{
    python_code_point::PythonCodePoint,
    python_json_load::python_json_loads,
    python_json_value::{PythonJsonObject, PythonJsonValue},
    python_string::PythonString,
    settings_strings::{SurrogateStandIns, python_string},
};

/// A string of `units`, each a UTF-16 code unit on its own, so a surrogate stays lone.
fn units(units: &[u16]) -> PythonString {
    let mut text = PythonString::new();
    text.extend(units.iter().copied().map(PythonCodePoint::from));
    text
}

#[test]
fn a_stand_in_turns_back_into_its_lone_surrogate_in_strings_and_keys() {
    let _strings = SurrogateStandIns::start();
    let exact = units(&[0x61, 0xDC00]);
    let stand_in = python_string(exact.clone()).expect("a stand-in");
    let document = Value::Object(Map::from_iter([(stand_in.clone(), json!([stand_in]))]));
    let value = js_exact_value(&document);
    let mut members = PythonJsonObject::new();
    members.insert(
        exact.clone(),
        PythonJsonValue::Array(vec![PythonJsonValue::String(exact)]),
    );
    assert_eq!(value, PythonJsonValue::Object(members));
    assert_eq!(js_stringify_exact(&value), r#"{"a\udc00":["a\udc00"]}"#);
}

#[test]
fn numbers_hold_the_double_javascript_prints() {
    let value = js_exact_value(&json!([1, -2, 1.5, 1e21, 9_007_199_254_740_993_u64]));
    assert_eq!(js_stringify_exact(&value), "[1,-2,1.5,1e+21,9007199254740992]");
    assert_eq!(value, python_json_loads("[1,-2,1.5,1e+21,9007199254740992]").unwrap());
}

#[test]
fn keys_print_in_javascript_enumeration_order() {
    let value = python_json_loads(r#"{"b":1,"1":2,"01":4,"0":3}"#).unwrap();
    assert_eq!(js_stringify_exact(&value), r#"{"0":3,"1":2,"b":1,"01":4}"#);
    let value = js_exact_value(&json!({"b": 1, "1": 2}));
    assert_eq!(js_stringify_exact(&value), r#"{"1":2,"b":1}"#);
}

#[test]
fn strings_escape_what_json_stringify_escapes() {
    let value = python_json_loads(r#"["\"\\\b\f\n\r\t\u0001\u007f \ud800😀"]"#).unwrap();
    assert_eq!(
        js_stringify_exact(&value),
        "[\"\\\"\\\\\\b\\f\\n\\r\\t\\u0001\u{7f}\u{2028}\\ud800\u{1f600}\"]"
    );
}

#[test]
fn integers_past_two_to_the_fifty_three_and_infinities_print_as_javascript_doubles() {
    let value = python_json_loads("[18446744073709551616, 1e400, -0.0, true, null]").unwrap();
    assert_eq!(js_stringify_exact(&value), "[18446744073709552000,null,0,true,null]");
}
