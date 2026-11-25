#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::json;

use super::{ExactInteger, PythonJsonObject, PythonJsonValue, PythonNumber, python_json_dumps};
use crate::python_c_stack::PythonCStack;
use crate::python_json_error::{ExactIntegerError, JsonDecodeErrorKind, PythonJsonError};
use crate::python_json_load::{drop_python_deep, python_json_loads};
use crate::python_string::PythonString;

fn round_trip(text: &str) -> String {
    python_json_dumps(&python_json_loads(text).unwrap())
}

#[test]
fn integers_of_any_size_stay_exact() {
    assert_eq!(
        round_trip("[123456789012345678901234567890, -98765432109876543210987654321, 18446744073709551616]"),
        "[123456789012345678901234567890, -98765432109876543210987654321, 18446744073709551616]"
    );
    let big = format!("[{}]", "9".repeat(400));
    assert_eq!(round_trip(&big), big);
}

#[test]
fn floats_keep_their_type_and_python_spelling() {
    assert_eq!(
        round_trip("[1.0, 1E5, -0, -0.0, 1e16, 1e15, 0.00001, 1e-400, -1e-400, 1e400, -1e400]"),
        "[1.0, 100000.0, 0, -0.0, 1e+16, 1000000000000000.0, 1e-05, 0.0, -0.0, Infinity, -Infinity]"
    );
}

#[test]
fn python_constants_read_and_print_as_python_spells_them() {
    assert_eq!(
        round_trip("[NaN, Infinity, -Infinity, null]"),
        "[NaN, Infinity, -Infinity, null]"
    );
    for text in [
        "[-NaN]",
        "[nan]",
        "[infinity]",
        "[01]",
        "[1.]",
        "[1e]",
        "[+1]",
        "[.5]",
        "[1,]",
        "{\"a\":1,}",
        "[1] 2",
    ] {
        assert!(
            matches!(python_json_loads(text), Err(PythonJsonError::Decode(_))),
            "{text}"
        );
    }
}

#[test]
fn a_repeated_key_keeps_its_first_position_and_last_value() {
    assert_eq!(round_trip(r#"{"a": 1, "b": 3, "a": 2}"#), r#"{"a": 2, "b": 3}"#);
}

#[test]
fn strings_print_with_ascii_escapes() {
    assert_eq!(
        round_trip(r#"["\u007fé😀", "\b\f\n\r\t\"\\/", "\u0001"]"#),
        r#"["\u007f\u00e9\ud83d\ude00", "\b\f\n\r\t\"\\/", "\u0001"]"#
    );
    assert_eq!(
        round_trip(r#"{"\udc00": "\ud800\ud83d\ude00\ud83d"}"#),
        r#"{"\udc00": "\ud800\ud83d\ude00\ud83d"}"#
    );
}

#[test]
fn integers_past_the_python_digit_limit_are_refused() {
    let error = python_json_loads(&format!("[-{}]", "1".repeat(4301))).unwrap_err();
    assert!(
        matches!(
            error,
            PythonJsonError::Integer(ExactIntegerError::TooManyDigits {
                limit: 4300,
                digits: 4301
            })
        ),
        "{error:?}"
    );
    assert!(python_json_loads(&format!("[{}]", "1".repeat(4300))).is_ok());
    assert_eq!(
        "0".repeat(4301).parse::<ExactInteger>(),
        Err(ExactIntegerError::TooManyDigits {
            limit: 4300,
            digits: 4301
        })
    );
}

#[test]
fn loads_nests_as_deep_as_json_loads_in_a_plain_python_call() -> Result<(), Box<dyn std::error::Error>> {
    let goldens: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/settings_option/python_settings.json"))?;
    let module = &goldens["depth"][0];
    let nested = |depth: usize| format!("{}null{}", "{\"a\":".repeat(depth), "}".repeat(depth));
    let deepest = PythonCStack::MODULE.json_depth();
    drop_python_deep(python_json_loads(&nested(deepest))?);
    let refused = python_json_loads(&nested(deepest + 1))
        .map(drop_python_deep)
        .map_err(|error| error.to_string());
    let expected = (
        Some("module"),
        Some(u64::try_from(deepest)?),
        Err(module["error"].as_str().map(str::to_owned)),
    );
    assert_eq!(
        (
            module["caller"].as_str(),
            module["loads"].as_u64(),
            refused.map_err(Some)
        ),
        expected
    );
    Ok(())
}

#[test]
fn a_byte_order_mark_is_refused_as_json_loads_refuses_it() {
    let error = python_json_loads("\u{feff}{}").unwrap_err();
    assert!(
        matches!(&error, PythonJsonError::Decode(error) if error.kind() == JsonDecodeErrorKind::ByteOrderMark && error.pos() == 0),
        "{error:?}"
    );
}

#[test]
fn exact_integers_parse_like_python_int() {
    let parsed: ExactInteger = "-000123".parse().unwrap();
    assert_eq!(parsed.as_str(), "-123");
    assert!(parsed.is_negative());
    assert_eq!(parsed.to_i64(), Some(-123));
    assert_eq!(parsed.to_u64(), None);
    assert_eq!("-0".parse::<ExactInteger>().unwrap().to_string(), "0");
    assert_eq!("+7".parse::<ExactInteger>().unwrap(), ExactInteger::from(7_u64));
    assert_eq!(
        "1.5".parse::<ExactInteger>(),
        Err(ExactIntegerError::Invalid("'1.5'".to_owned()))
    );
    assert_eq!(
        "18446744073709551616".parse::<ExactInteger>().unwrap().to_f64(),
        18_446_744_073_709_551_616.0
    );
}

#[test]
fn serde_json_values_convert_with_their_number_types() {
    let value = PythonJsonValue::from(&json!({"i": -3, "u": 18_446_744_073_709_551_615_u64, "f": 2.0, "s": "x"}));
    assert_eq!(
        python_json_dumps(&value),
        r#"{"i": -3, "u": 18446744073709551615, "f": 2.0, "s": "x"}"#
    );
    assert!(PythonNumber::Float(0.0).is_identical(&PythonNumber::Float(0.0)));
    assert!(!PythonNumber::Float(0.0).is_identical(&PythonNumber::Float(-0.0)));
    assert!(PythonNumber::Float(f64::NAN).is_identical(&PythonNumber::Float(f64::NAN)));
    assert!(!PythonNumber::Int(ExactInteger::from(1_u64)).is_identical(&PythonNumber::Float(1.0)));
}

#[test]
fn accessors_answer_as_serde_json_value_answers() -> Result<(), Box<dyn std::error::Error>> {
    let value = PythonJsonValue::from(&json!({
        "n": null, "b": true, "i": -3, "u": 7, "f": 2.5, "s": "x", "a": [1], "o": {"k": 1}
    }));
    let mut kinds = Vec::new();
    for key in ["n", "b", "i", "f", "s", "a", "o"] {
        let member = value.get(key).ok_or("a member is missing")?;
        kinds.push([
            member.is_null(),
            member.is_boolean(),
            member.is_number(),
            member.is_string(),
            member.is_array(),
            member.is_object(),
        ]);
    }
    let one_hot = |at: usize| std::array::from_fn::<bool, 6, _>(|index| index == at);
    assert_eq!(kinds, [0, 1, 2, 2, 3, 4, 5].map(one_hot));
    let number = |key: &str| value.get(key).and_then(PythonJsonValue::as_number);
    assert_eq!(
        [number("i"), number("u"), number("f")].map(|n| n.map(|n| (n.is_f64(), n.as_i64(), n.as_u64()))),
        [
            Some((false, Some(-3), None)),
            Some((false, Some(7), Some(7))),
            Some((true, None, None))
        ]
    );
    assert_eq!(
        (
            value.get("b").and_then(PythonJsonValue::as_bool),
            value.get("s").and_then(PythonJsonValue::as_str),
            value.get("a").and_then(PythonJsonValue::as_array).map(Vec::len),
            value.get("o").and_then(|members| members.get("k")),
        ),
        (
            Some(true),
            Some("x"),
            Some(1),
            Some(&PythonJsonValue::Number(PythonNumber::Int(ExactInteger::from(1_u64))))
        )
    );
    let null = &PythonJsonValue::Null;
    assert_eq!(
        (
            null.as_bool(),
            null.as_number(),
            null.as_python_str(),
            null.as_array(),
            null.as_object(),
            null.get("k")
        ),
        (None, None, None, None, None, None)
    );
    let surrogate = python_json_loads(r#""\ud800""#)?;
    assert_eq!(
        (
            surrogate.as_python_str().map(|text| text.code_points().count()),
            surrogate.as_str()
        ),
        (Some(1), None)
    );
    Ok(())
}

#[test]
fn nested_serde_json_values_convert_in_order() {
    let value = PythonJsonValue::from(json!({"a": [1, {"b": [], "c": {}}, "d"], "e": {"f": [null, [true]]}}));
    assert_eq!(
        python_json_dumps(&value),
        r#"{"a": [1, {"b": [], "c": {}}, "d"], "e": {"f": [null, [true]]}}"#
    );
    let mut deep = serde_json::Value::Null;
    for _ in 0..1000 {
        deep = serde_json::Value::Array(vec![deep]);
    }
    assert_eq!(
        python_json_dumps(&PythonJsonValue::from(&deep)),
        format!("{}null{}", "[".repeat(1000), "]".repeat(1000))
    );
}

#[test]
fn a_bool_converts_to_the_value_json_dumps_writes_for_it() {
    assert_eq!(python_json_dumps(&PythonJsonValue::from(true)), "true");
    assert_eq!(python_json_dumps(&PythonJsonValue::from(false)), "false");
}

#[test]
fn only_an_object_lends_its_members_for_writing() {
    let mut object = PythonJsonValue::Object(PythonJsonObject::new());
    if let Some(members) = object.as_object_mut() {
        members.insert(PythonString::from("a"), PythonJsonValue::Null);
    }
    assert_eq!(python_json_dumps(&object), "{\"a\": null}");
    assert!(PythonJsonValue::Null.as_object_mut().is_none());
    assert!(PythonJsonValue::Array(Vec::new()).as_object_mut().is_none());
}
