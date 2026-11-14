//! Replays `tests/fixtures/sdk_mcp/python_request_ids.json`: what `json.dumps(json.loads(token))` printed for each
//! id token under Python 3 with `claude_agent_sdk` 0.2.181 and `mcp` 2.3.0.

use super::*;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_request_ids.json");

fn cases() -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    Ok(fixture
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("missing cases")?
        .clone())
}

fn text<'a>(case: &'a Value, field: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    Ok(case.get(field).and_then(Value::as_str).ok_or("missing field")?)
}

#[test]
fn every_recorded_token_is_written_back_as_python_wrote_it() -> TestResult {
    for case in cases()? {
        let token = text(&case, "token")?;
        let raw = RawJsonId::parse(token)?;
        assert_eq!(raw.python_dump(), text(&case, "dump")?, "token {token:.40}");
        let is_int = case.get("is_int").and_then(Value::as_bool).ok_or("missing is_int")?;
        assert_eq!(raw.integer().is_some(), is_int, "token {token:.40}");
        if is_int {
            assert_eq!(raw.integer(), Some(raw.python_dump()));
        }
        assert_eq!(raw.is_string(), token.starts_with('"'), "token {token:.40}");
    }
    Ok(())
}

#[test]
fn integers_beyond_64_bits_keep_every_digit_and_negative_zero_is_zero() -> TestResult {
    assert_eq!(
        RawJsonId::parse("123456789012345678901234567890")?.integer(),
        Some("123456789012345678901234567890")
    );
    assert_eq!(
        RawJsonId::parse("-1180591620717411303424")?.integer(),
        Some("-1180591620717411303424")
    );
    assert_eq!(RawJsonId::parse("-0")?.integer(), Some("0"));
    assert_eq!(RawJsonId::parse("1E2")?.integer(), None);
    assert_eq!(RawJsonId::parse(" 7\n")?.integer(), Some("7"));
    Ok(())
}

#[test]
fn text_python_refuses_is_refused() {
    for token in [
        "",
        "00",
        "01",
        "+1",
        "1.",
        "0x10",
        "-",
        "1e",
        "\"x",
        "[1,]",
        "{\"a\"}",
        "tru",
        "1 2",
        "\"\u{1}\"",
    ] {
        assert_eq!(RawJsonId::parse(token), Err(RawJsonIdError::Invalid), "token {token:?}");
    }
}

#[test]
fn an_integer_past_the_python_digit_limit_is_refused() -> TestResult {
    let limit = format!("1{}", "0".repeat(4299));
    assert_eq!(RawJsonId::parse(&limit)?.integer(), Some(limit.as_str()));
    let over = format!("-1{}", "0".repeat(4300));
    assert_eq!(
        RawJsonId::parse(&over),
        Err(RawJsonIdError::IntegerTooLong { digits: 4301 })
    );
    Ok(())
}

#[test]
fn a_repeated_key_keeps_its_first_position_and_its_last_value() -> TestResult {
    let raw = RawJsonId::parse(r#"{"a":1,"b":2,"a":3}"#)?;
    assert_eq!(raw.python_dump(), r#"{"a": 3, "b": 2}"#);
    Ok(())
}

#[test]
fn escaped_lone_surrogates_are_written_as_python_keeps_them() -> TestResult {
    assert_eq!(
        RawJsonId::parse(r#""\ud800x\u007f/""#)?.python_dump(),
        r#""\ud800x\u007f/""#
    );
    Ok(())
}
