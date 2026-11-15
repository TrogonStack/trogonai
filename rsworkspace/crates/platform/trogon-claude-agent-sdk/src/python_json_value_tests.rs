#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::json;

use super::{
    ExactInteger, ExactIntegerError, PythonJsonError, PythonJsonValue, PythonNumber, python_json_dumps,
    python_json_loads,
};

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
            matches!(python_json_loads(text), Err(PythonJsonError::Syntax(_))),
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
fn a_byte_order_mark_is_refused_as_json_loads_refuses_it() {
    let error = python_json_loads("\u{feff}{}").unwrap_err();
    assert!(matches!(error, PythonJsonError::ByteOrderMark), "{error:?}");
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
