#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "The tests build the JSON values the transport parses.")
)]

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const HELPERS: &str = include_str!("../tests/fixtures/sse_frames/helpers.json");

fn section(name: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let helpers: Value = serde_json::from_str(HELPERS)?;
    Ok(helpers
        .get(name)
        .and_then(Value::as_array)
        .ok_or("missing section")?
        .clone())
}

fn parsed(entry: &Value) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    match entry.get("json").and_then(Value::as_str) {
        Some(text) => Ok(Some(serde_json::from_str(text)?)),
        None => Ok(None),
    }
}

#[test]
fn whitespace_matches_the_ecmascript_class() {
    for ch in [
        '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{a0}', '\u{1680}', '\u{2003}', '\u{2028}', '\u{2029}', '\u{202f}',
        '\u{205f}', '\u{3000}', '\u{feff}',
    ] {
        assert!(is_js_whitespace(ch), "{ch:?}");
    }
    for ch in ['a', '0', '\u{85}', '\u{200b}', '\u{180e}'] {
        assert!(!is_js_whitespace(ch), "{ch:?}");
    }
}

#[test]
fn sequence_numbers_match_the_browser_parser() -> TestResult {
    for entry in section("ob")? {
        let value = parsed(&entry)?;
        let expected = entry.get("result").and_then(Value::as_u64);
        assert_eq!(sequence_number(value.as_ref()), Ok(expected), "{entry}");
    }
    Ok(())
}

#[test]
fn string_conversion_matches_the_browser_log_text() -> TestResult {
    for entry in section("strings")? {
        let text = match parsed(&entry)? {
            Some(value) => js_to_string(&value),
            None => Ok("undefined".to_owned()),
        };
        let expected = entry.get("tu").and_then(Value::as_str).ok_or("missing tu")?;
        assert_eq!(
            text.map(|text| sanitize_log_text(&text)),
            Ok(expected.to_owned()),
            "{entry}"
        );
    }
    Ok(())
}

#[test]
fn sanitizing_and_truncating_match_the_browser() -> TestResult {
    for entry in section("sanitize")? {
        let text = entry.get("text").and_then(Value::as_str).ok_or("missing text")?;
        if let Some(expected) = entry.get("db").and_then(Value::as_str) {
            assert_eq!(sanitize_log_text(text), expected, "{entry}");
        } else {
            let limit = entry.get("limit").and_then(Value::as_i64).ok_or("missing limit")?;
            let expected = entry.get("xr").and_then(Value::as_str).ok_or("missing xr")?;
            assert_eq!(
                truncate_utf16(text, usize::try_from(limit).unwrap_or(0)),
                expected,
                "{entry}"
            );
        }
    }
    Ok(())
}

#[test]
fn objects_with_their_own_to_string_cannot_be_converted() -> TestResult {
    let object: Value = serde_json::from_str(r#"{"toString":1}"#)?;
    assert_eq!(js_to_string(&object), Err(JsConversionError));
    let nested: Value = serde_json::from_str(r#"[1,{"toString":1}]"#)?;
    assert_eq!(js_to_string(&nested), Err(JsConversionError));
    assert_eq!(sequence_number(Some(&nested)), Err(JsConversionError));
    Ok(())
}

#[test]
fn arrays_and_scalars_convert_like_string() -> TestResult {
    let array: Value = serde_json::from_str(r#"[1,null,"a",[true,{}],2.5]"#)?;
    assert_eq!(js_to_string(&array).as_deref(), Ok("1,,a,true,[object Object],2.5"));
    assert_eq!(js_to_string(&Value::Bool(false)).as_deref(), Ok("false"));
    Ok(())
}

#[test]
fn lone_surrogate_escapes_are_replaced_and_pairs_kept() -> TestResult {
    let text = r#"["\ud83d\ude00","\\ud800","\ud800\u0041","\udc00"]"#;
    let value = parse_json(text, "\\ufffd")?;
    let items = value.as_array().ok_or("not a list")?;
    let texts: Vec<_> = items.iter().filter_map(Value::as_str).collect();
    assert_eq!(texts, ["\u{1f600}", "\\ud800", "\u{fffd}A", "\u{fffd}"]);
    Ok(())
}

#[test]
fn text_that_is_not_json_stays_an_error() {
    assert!(parse_json("nope", "\\ufffd").is_err());
    assert!(parse_json(r#""\u12"#, "\\ufffd").is_err());
    assert!(parse_json(r#""\ud800"#, "\\ufffd").is_err());
    assert!(parse_json(r#"{"a":1}"#, "\\ufffd").is_ok());
}

#[test]
fn truncation_does_not_split_a_surrogate_pair() {
    assert_eq!(truncate_utf16("a\u{1f600}b", 2), "a");
    assert_eq!(truncate_utf16("a\u{1f600}b", 3), "a\u{1f600}");
}
