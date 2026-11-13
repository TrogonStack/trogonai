use super::*;

/// `json.dumps(value)` as printed by CPython 3.12.
const PYTHON_FLOATS: &[(f64, &str)] = &[
    (1.5, "1.5"),
    (30.0, "30.0"),
    (1e-7, "1e-07"),
    (1e16, "1e+16"),
    (1e15, "1000000000000000.0"),
    (123_456_789_012_345_680.0, "1.2345678901234568e+17"),
    (0.0001, "0.0001"),
    (0.00001, "1e-05"),
    (1e22, "1e+22"),
    (5e-324, "5e-324"),
    (f64::MAX, "1.7976931348623157e+308"),
    (-0.0, "-0.0"),
    (2.5e-5, "2.5e-05"),
    (12345.678, "12345.678"),
];

#[test]
fn floats_follow_python_repr() {
    for (value, expected) in PYTHON_FLOATS {
        assert_eq!(float_repr(*value), *expected);
    }
}

#[test]
fn objects_use_spaced_separators_and_ascii_escapes() -> Result<(), serde_json::Error> {
    let value = serde_json::json!({"a": "\u{e9}\u{1f600}\u{7f}\u{1f}\u{2028}", "b": [1, 2.5]});
    assert_eq!(
        to_string(&value)?,
        r#"{"a": "\u00e9\ud83d\ude00\u007f\u001f\u2028", "b": [1, 2.5]}"#
    );
    Ok(())
}
