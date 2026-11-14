use super::*;
use crate::{
    constants::{JSON_DEPTH, PYTHON_INT_MAX_STR_DIGITS},
    python_json_error::ExactIntegerError,
    python_json_value::python_json_dumps,
    python_urlopen_text::{a2b_base64, bytes_repr},
};
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// `json.loads` of each recorded document, from CPython 3.12 run on its own rather than inside the bridge.
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_remote.json");

/// How deep a standalone `json.loads` nests: the fixture records 9991 nested arrays loading there.
const STANDALONE_DEPTH: usize = 9991;

/// The recorded bytes: base64, or `[base64, repeat]` chunks for long documents.
fn fixture_bytes(recorded: &Value) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if let Some(text) = recorded.as_str() {
        return Ok(a2b_base64(text.as_bytes())?);
    }
    let chunks = recorded.as_array().ok_or("bytes are neither base64 nor chunks")?;
    let mut bytes = Vec::new();
    for chunk in chunks {
        let text = chunk.get(0).and_then(Value::as_str).ok_or("a chunk has no base64")?;
        let count = usize::try_from(chunk.get(1).and_then(Value::as_u64).ok_or("a chunk has no repeat")?)?;
        bytes.extend(a2b_base64(text.as_bytes())?.repeat(count));
    }
    Ok(bytes)
}

/// The depth of `[[...]]` text, which the golden compares by depth rather than by text.
fn bracket_depth(text: &str) -> Option<usize> {
    let depth = text.bytes().take_while(|&byte| byte == b'[').count();
    (depth > 100 && text.len() == depth * 2 && text.bytes().skip(depth).all(|byte| byte == b']')).then_some(depth)
}

/// How many arrays nest in `value` when each holds at most the next, walked without recursion.
fn nested_python_arrays(value: &PythonJsonValue) -> Option<usize> {
    let mut depth = 0;
    let mut single = true;
    let mut at = Some(value);
    while let Some(PythonJsonValue::Array(items)) = at {
        depth += 1;
        single &= items.len() <= 1;
        at = items.first();
    }
    single.then_some(depth)
}

fn nested(opening: &str, innermost: &str, closing: &str, depth: usize) -> String {
    [opening.repeat(depth), innermost.to_owned(), closing.repeat(depth)].concat()
}

/// `json.loads(data)` for bytes, with what Python raises named as the fixture names it.
fn load(data: &[u8], max_depth: usize) -> Result<PythonJsonValue, &'static str> {
    let text = decode(data).ok_or("UnicodeDecodeError")?;
    parse(&text, max_depth).map_err(|error| match error {
        PythonJsonError::Decode(_) => "JSONDecodeError",
        PythonJsonError::Recursion(_) => "RecursionError",
        PythonJsonError::Integer(_) => "ValueError",
    })
}

/// `json.dumps(json.loads(data))`, or the name of what Python raises.
fn dumped(data: &[u8]) -> Result<String, &'static str> {
    load(data, JSON_DEPTH).map(|value| python_json_dumps(&value))
}

/// What and where the `JSONDecodeError` that `json.loads(text)` raises for a `str` reports, if it raises one.
fn located(text: &str) -> Option<(JsonDecodeErrorKind, usize, usize, usize)> {
    match loads_text(text, JSON_DEPTH) {
        Err(PythonJsonError::Decode(error)) => Some((error.kind(), error.pos(), error.lineno(), error.colno())),
        _ => None,
    }
}

/// `json.dumps(json.loads(text))` for a `str`.
fn dumped_text(text: &str) -> Result<String, PythonJsonError> {
    loads_text(text, JSON_DEPTH).map(|value| python_json_dumps(&value))
}

#[test]
fn loads_matches_python_for_every_golden_document() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let cases = fixture.get("json").and_then(Value::as_array).ok_or("no json cases")?;
    assert!(!cases.is_empty());
    for case in cases {
        let data = fixture_bytes(case.get("bytes").ok_or("a case has no bytes")?)?;
        let label = bytes_repr(data.get(..data.len().min(40)).unwrap_or_default());
        let loaded = load(&data, STANDALONE_DEPTH);
        if let Some(name) = case.get("error").and_then(Value::as_str) {
            assert_eq!(loaded.err(), Some(name), "{label}");
            continue;
        }
        let text = case.get("value").and_then(Value::as_str).ok_or("a case has no value")?;
        let loaded = loaded.map_err(|error| format!("{label}: {error}"))?;
        let unrepresentable = case.get("unrepresentable").and_then(Value::as_bool) == Some(true);
        if let Some(depth) = bracket_depth(text) {
            assert_eq!(nested_python_arrays(&loaded), Some(depth), "{label}");
            drop_python_deep(loaded);
            continue;
        }
        assert_eq!(python_json_dumps(&loaded), text, "{label}");
        if unrepresentable {
            continue;
        }
        let expected: Value = serde_json::from_str(text)?;
        assert_eq!(loaded, PythonJsonValue::from(&expected), "{label}");
    }
    Ok(())
}

#[test]
fn loads_is_none_when_python_raises() {
    let high = || PythonString::from(PythonCodePoint::from(0xd800_u16));
    let cases: [(&[u8], Option<PythonJsonValue>); 7] = [
        (
            br#"{"a": [1, -2, 2.5, "x", true, null]}"#,
            serde_json::from_str::<Value>(r#"{"a": [1, -2, 2.5, "x", true, null]}"#)
                .ok()
                .map(PythonJsonValue::from),
        ),
        (b"\xff\xfe{\x00}", None),
        (b"[1,]", None),
        (b"[[[1]]]", None),
        (
            b"[Infinity, 18446744073709551616]",
            "18446744073709551616"
                .parse::<crate::python_json_value::ExactInteger>()
                .ok()
                .map(|big| {
                    PythonJsonValue::Array(vec![
                        PythonJsonValue::from(f64::INFINITY),
                        PythonJsonValue::Number(PythonNumber::Int(big)),
                    ])
                }),
        ),
        (
            br#"["\ud800"]"#,
            Some(PythonJsonValue::Array(vec![PythonJsonValue::String(high())])),
        ),
        (
            br#"{"\ud800": 1}"#,
            Some(PythonJsonValue::Object(IndexMap::from([(
                high(),
                PythonJsonValue::from(1),
            )]))),
        ),
    ];
    let actual: Vec<Option<PythonJsonValue>> = cases.iter().map(|(data, _)| loads(data, 2)).collect();
    let expected: Vec<Option<PythonJsonValue>> = cases.into_iter().map(|(_, value)| value).collect();
    assert_eq!(actual, expected);
}

#[test]
fn loads_raises_recursion_error_past_the_bridge_depth() -> TestResult {
    let deepest = load(nested("[", "", "]", JSON_DEPTH).as_bytes(), JSON_DEPTH)?;
    let found = nested_python_arrays(&deepest);
    drop_python_deep(deepest);
    assert_eq!(found, Some(JSON_DEPTH));
    let too_deep = nested("[", "", "]", JSON_DEPTH + 1);
    assert_eq!(
        loads_text(&too_deep, JSON_DEPTH).err(),
        Some(PythonJsonError::Recursion(JsonContainer::Array))
    );
    let objects_too_deep = nested("{\"a\": [", "{}", "]}", JSON_DEPTH / 2);
    assert_eq!(
        loads_text(&objects_too_deep, JSON_DEPTH).err(),
        Some(PythonJsonError::Recursion(JsonContainer::Object))
    );
    assert_eq!(
        load(objects_too_deep.as_bytes(), JSON_DEPTH).err(),
        Some("RecursionError")
    );
    let objects = loads(nested("{\"a\": ", "{}", "}", 5000).as_bytes(), JSON_DEPTH).ok_or("5000 objects load")?;
    assert!(objects.get("a").is_some_and(PythonJsonValue::is_object));
    drop_python_deep(objects);
    let surrogate = nested("{\"a\": ", "\"\\ud800\"", "}", 5000);
    let surrogate = loads(surrogate.as_bytes(), JSON_DEPTH).ok_or("a lone surrogate 5000 objects deep loads")?;
    drop_python_deep(surrogate);
    let exotic = nested("{\"a\": ", "[NaN, -Infinity, -18446744073709551617]", "}", 5000);
    let exotic = loads(exotic.as_bytes(), JSON_DEPTH).ok_or("exotic numbers load")?;
    drop_python_deep(exotic);
    let trailing = nested("[", "", "]", JSON_DEPTH) + " x";
    let end = JSON_DEPTH * 2 + 1;
    assert_eq!(
        located(&trailing),
        Some((JsonDecodeErrorKind::ExtraData, end, 1, end + 1))
    );
    let unclosed = "[".repeat(JSON_DEPTH);
    assert_eq!(load(unclosed.as_bytes(), JSON_DEPTH).err(), Some("JSONDecodeError"));
    Ok(())
}

#[test]
fn loads_keeps_the_first_position_and_the_last_value_of_a_repeated_key() -> TestResult {
    assert_eq!(dumped_text(r#"{"b": 1, "a": 2, "b": [3]}"#)?, r#"{"b": [3], "a": 2}"#);
    Ok(())
}

#[test]
fn loads_decodes_with_the_encoding_json_detects() {
    let cases: [(&[u8], Result<&str, &str>); 25] = [
        (b"\xff\xfe\x00\x00[\x00\x00\x00]\x00\x00\x00", Ok("[]")),
        (b"\x00\x00\xfe\xff\x00\x00\x00[\x00\x00\x00]", Ok("[]")),
        (b"\xff\xfe\"\x00\x3d\xd8\x00\xde\"\x00", Ok("\"\\ud83d\\ude00\"")),
        (b"\xfe\xff\x00\"\xd8\x3d\xde\x00\x00\"", Ok("\"\\ud83d\\ude00\"")),
        (b"\xef\xbb\xbf[]", Ok("[]")),
        (b"\x00\x00\x001", Ok("1")),
        (b"\x00[\x00]", Ok("[]")),
        (b"1\x00\x00\x00", Ok("1")),
        (b"[\x00]\x00", Ok("[]")),
        (b"\x001", Ok("1")),
        (b"1\x00", Ok("1")),
        (b"[1]", Ok("[1]")),
        (b"\xff\xfe\"\x00\x3d\xd8\"\x00", Ok("\"\\ud83d\"")),
        (
            b"\xff\xfe\"\x00\x3d\xd8\x3d\xd8\x00\xde\"\x00",
            Ok("\"\\ud83d\\ud83d\\ude00\""),
        ),
        (b"\xff\xfe\"\x00\x00\xde\x3d\xd8\"\x00", Ok("\"\\ude00\\ud83d\"")),
        (b"\xff\xfe1\x00\x3d\xd8", Err("JSONDecodeError")),
        (b"\xff\xfe{\x00}", Err("UnicodeDecodeError")),
        (b"\xff\xfe\x3d\xd8\x41", Err("UnicodeDecodeError")),
        (b"\xff\xfe\x00\x00{\x00\x00\x00}\x00\x00", Err("UnicodeDecodeError")),
        (b"\xff\xfe\x00\x00\x00\x00\x11\x00", Err("UnicodeDecodeError")),
        (b"\xef\xbb\xbf\xed\xa0", Err("UnicodeDecodeError")),
        (b"\"\xed\xa0\x80\xed\xb0\x80\"", Ok("\"\\ud800\\udc00\"")),
        (b"[\"\xc3\xa9\xed\xa0\x80\"]", Ok("[\"\\u00e9\\ud800\"]")),
        (b"\xef\xbb\xbf\xef\xbb\xbf[]", Err("JSONDecodeError")),
        (b"\xf4\x90\x80\x80", Err("UnicodeDecodeError")),
    ];
    let actual: Vec<Result<String, &str>> = cases.iter().map(|(data, _)| dumped(data)).collect();
    let expected: Vec<Result<String, &str>> = cases
        .into_iter()
        .map(|(_, outcome)| outcome.map(str::to_owned))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn loads_raises_json_decode_error_where_the_c_scanner_does() {
    let cases: [(&str, JsonDecodeErrorKind, usize, usize, usize); 28] = [
        ("", JsonDecodeErrorKind::ExpectingValue, 0, 1, 1),
        (" ", JsonDecodeErrorKind::ExpectingValue, 1, 1, 2),
        ("-", JsonDecodeErrorKind::ExpectingValue, 0, 1, 1),
        ("-x", JsonDecodeErrorKind::ExpectingValue, 0, 1, 1),
        ("01", JsonDecodeErrorKind::ExtraData, 1, 1, 2),
        ("1.", JsonDecodeErrorKind::ExtraData, 1, 1, 2),
        ("1.5e", JsonDecodeErrorKind::ExtraData, 3, 1, 4),
        ("1e+", JsonDecodeErrorKind::ExtraData, 1, 1, 2),
        ("[1,]", JsonDecodeErrorKind::ExpectingValue, 3, 1, 4),
        ("[1 2]", JsonDecodeErrorKind::ExpectingComma, 3, 1, 4),
        ("{1: 2}", JsonDecodeErrorKind::ExpectingPropertyName, 1, 1, 2),
        ("{\"a\" 1}", JsonDecodeErrorKind::ExpectingColon, 5, 1, 6),
        ("{\"a\": 1,}", JsonDecodeErrorKind::ExpectingPropertyName, 8, 1, 9),
        ("{\"a\": 1 \"b\": 2}", JsonDecodeErrorKind::ExpectingComma, 8, 1, 9),
        ("\"abc", JsonDecodeErrorKind::UnterminatedString, 0, 1, 1),
        ("\"\\", JsonDecodeErrorKind::UnterminatedString, 0, 1, 1),
        ("\"\\x\"", JsonDecodeErrorKind::InvalidEscape, 1, 1, 2),
        ("\"\\u12\"", JsonDecodeErrorKind::InvalidUnicodeEscape, 2, 1, 3),
        ("\"\\u12g4\"", JsonDecodeErrorKind::InvalidUnicodeEscape, 2, 1, 3),
        ("\"a\tb\"", JsonDecodeErrorKind::InvalidControlCharacter, 2, 1, 3),
        ("nul", JsonDecodeErrorKind::ExpectingValue, 0, 1, 1),
        ("[1] [2]", JsonDecodeErrorKind::ExtraData, 4, 1, 5),
        ("\u{feff}{}", JsonDecodeErrorKind::ByteOrderMark, 0, 1, 1),
        ("\n\n  x", JsonDecodeErrorKind::ExpectingValue, 4, 3, 3),
        ("\"\\ud800\\u12G4\"", JsonDecodeErrorKind::InvalidUnicodeEscape, 8, 1, 9),
        ("[[]", JsonDecodeErrorKind::ExpectingComma, 3, 1, 4),
        ("{\"a\":{}", JsonDecodeErrorKind::ExpectingComma, 7, 1, 8),
        ("\"\\u1234", JsonDecodeErrorKind::InvalidUnicodeEscape, 2, 1, 3),
    ];
    let actual: Vec<_> = cases.iter().map(|(text, ..)| located(text)).collect();
    let expected: Vec<_> = cases
        .iter()
        .map(|(_, kind, pos, lineno, colno)| Some((*kind, *pos, *lineno, *colno)))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(located("[1]"), None);
}

#[test]
fn loads_keeps_a_lone_surrogate_as_python_does() -> TestResult {
    let cases = [
        ("\"\\ud800\"", "\"\\ud800\""),
        ("\"\\udc00\\ud800\"", "\"\\udc00\\ud800\""),
        ("{\"\\ud800\":1}", "{\"\\ud800\": 1}"),
        ("\"\\ud83d\\u0041\"", "\"\\ud83dA\""),
        ("\"\\ud83d\\ude00\"", "\"\\ud83d\\ude00\""),
        ("\"\\ud83d\\ude00\\ud83d\"", "\"\\ud83d\\ude00\\ud83d\""),
        ("\"\\ud83d\\ud83d\\ude00\"", "\"\\ud83d\\ud83d\\ude00\""),
    ];
    let actual: Vec<String> = cases
        .iter()
        .map(|(text, _)| dumped_text(text))
        .collect::<Result<_, _>>()?;
    let expected: Vec<&str> = cases.iter().map(|(_, dumps)| *dumps).collect();
    assert_eq!(actual, expected);
    let pair = loads_text("\"\\ud83d\\ude00\"", JSON_DEPTH)?;
    assert_eq!(pair, PythonJsonValue::String(PythonString::from("\u{1F600}")));
    Ok(())
}

#[test]
fn loads_reads_every_scalar_python_reads() -> TestResult {
    let cases = [
        ("null", "null"),
        ("true", "true"),
        ("false", "false"),
        ("NaN", "NaN"),
        ("Infinity", "Infinity"),
        ("-Infinity", "-Infinity"),
        ("-0", "0"),
        ("-12", "-12"),
        ("123456789012345678901234567890", "123456789012345678901234567890"),
        ("1.5", "1.5"),
        ("1E2", "100.0"),
        ("2e-1", "0.2"),
        ("1e400", "Infinity"),
        ("\"\\\"\\\\\\/\\b\\f\\n\\r\\t\"", "\"\\\"\\\\/\\b\\f\\n\\r\\t\""),
        (" [ ] ", "[]"),
        (" { } ", "{}"),
    ];
    let actual: Vec<String> = cases
        .iter()
        .map(|(text, _)| dumped_text(text))
        .collect::<Result<_, _>>()?;
    let expected: Vec<&str> = cases.iter().map(|(_, dumps)| *dumps).collect();
    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn loads_rejects_an_integer_past_the_digit_limit() {
    let digits = format!("-{}", "7".repeat(PYTHON_INT_MAX_STR_DIGITS + 1));
    assert_eq!(
        loads_text(&digits, JSON_DEPTH).err(),
        Some(PythonJsonError::Integer(ExactIntegerError::TooManyDigits {
            limit: PYTHON_INT_MAX_STR_DIGITS,
            digits: PYTHON_INT_MAX_STR_DIGITS + 1,
        }))
    );
}
