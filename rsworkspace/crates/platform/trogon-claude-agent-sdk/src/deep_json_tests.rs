#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests build the values they walk inline.")
)]

use super::*;
use crate::{ContentBlock, Message, ToolName, ToolResultStatus, ToolUseId};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Deeper than the native stack of a test thread holds a recursive walk over.
const DEEP: usize = 200_000;

/// `leaf` wrapped in `depth` arrays, or objects under the key `a`, built without recursion.
fn nested(depth: usize, objects: bool, leaf: Value) -> Value {
    let mut value = leaf;
    for _ in 0..depth {
        value = if objects {
            Value::Object(JsonObject::from_iter([("a".to_owned(), value)]))
        } else {
            Value::Array(vec![value])
        };
    }
    value
}

/// `left == right` at any nesting: arrays equal item by item, objects key by key in any order, and numbers as
/// `serde_json` compares them.
fn values_equal(left: &Value, right: &Value) -> bool {
    let mut pending = vec![(left, right)];
    while let Some(pair) = pending.pop() {
        match pair {
            (Value::Array(left), Value::Array(right)) => {
                if left.len() != right.len() {
                    return false;
                }
                pending.extend(left.iter().zip(right));
            }
            (Value::Object(left), Value::Object(right)) => {
                if left.len() != right.len() {
                    return false;
                }
                for (key, value) in left {
                    let Some(other) = right.get(key) else {
                        return false;
                    };
                    pending.push((value, other));
                }
            }
            (Value::Array(_) | Value::Object(_), _) | (_, Value::Array(_) | Value::Object(_)) => return false,
            (left, right) => {
                if left != right {
                    return false;
                }
            }
        }
    }
    true
}

/// A piece of the compact text of a value still to be written.
enum Piece<'a> {
    Value(&'a Value),
    Key(&'a str),
    Text(&'static str),
}

/// `serde_json::to_string(value)` at any nesting: the compact text, with every scalar and key written by the
/// `Display` of `serde_json` itself.
fn to_string(value: &Value) -> String {
    let mut text = String::new();
    let mut pending = vec![Piece::Value(value)];
    while let Some(piece) = pending.pop() {
        match piece {
            Piece::Text(literal) => text.push_str(literal),
            Piece::Key(key) => text.push_str(&Value::from(key).to_string()),
            Piece::Value(Value::Array(items)) => {
                pending.push(Piece::Text("]"));
                for (index, item) in items.iter().enumerate().rev() {
                    pending.push(Piece::Value(item));
                    if index > 0 {
                        pending.push(Piece::Text(","));
                    }
                }
                pending.push(Piece::Text("["));
            }
            Piece::Value(Value::Object(object)) => {
                pending.push(Piece::Text("}"));
                for (index, (key, item)) in object.iter().enumerate().rev() {
                    pending.push(Piece::Value(item));
                    pending.push(Piece::Text(":"));
                    pending.push(Piece::Key(key));
                    if index > 0 {
                        pending.push(Piece::Text(","));
                    }
                }
                pending.push(Piece::Text("{"));
            }
            Piece::Value(scalar) => text.push_str(&scalar.to_string()),
        }
    }
    text
}

#[test]
fn the_nested_reader_reads_and_rejects_what_serde_json_does() {
    let texts: [&[u8]; 34] = [
        b"{}",
        b" \t\r\n{ \"a\" : [ ] , \"b\" :{ } } \n",
        br#"{"a":1,"a":2}"#,
        br#"{"b":1,"a":2,"b":3}"#,
        r#"{"s":"x\"\\\/é😀","t":"\\"}"#.as_bytes(),
        br#"{"n":[-0,-0.0,1E2,1e-400,18446744073709551616,-9223372036854775809,true,false,null]}"#,
        br#"{"a":[[1,[]],{"b":{"c":[{}]}}]}"#,
        br#"{"n":1e400}"#,
        br#"{"n":01}"#,
        br#"{"n":+1}"#,
        br#"{"n":.5}"#,
        br#"{"a":[1,]}"#,
        br#"{"a":1,}"#,
        br#"{"a" 1}"#,
        br#"{1:2}"#,
        br#"{"a":1"b":2}"#,
        b"[1]",
        b"1",
        b"\"x\"",
        br#"{"a":1} x"#,
        br#"{"a":1}}"#,
        br#"{"a":tru}"#,
        br#"{"a":NaN}"#,
        br#"{"a":"\ud800"}"#,
        br#"{"a":"\x"}"#,
        b"{\"a\":\"\x01\"}",
        br#"{"a":"unterminated"#,
        br#"{"a":[1 2]}"#,
        br#"{"a":{"b":1]}"#,
        br#"{"a":[1}"#,
        b"{",
        b"",
        br#"{"a":}"#,
        br#"{"a":[{"b":"#,
    ];
    for text in texts {
        assert_eq!(
            format!("{:?}", parse_object(text)),
            format!("{:?}", serde_json::from_slice::<JsonObject>(text)),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
}

#[test]
fn a_shallow_message_parses_like_serde_json() -> TestResult {
    let text = br#"{"type": "x", "v": [1, 2.5, {"b": null}]}"#;
    assert_eq!(parse_object(text)?, serde_json::from_slice::<JsonObject>(text)?);
    let corrupt = br#"{"type": "#;
    assert_eq!(
        parse_object(corrupt).map_err(|error| error.to_string()),
        serde_json::from_slice::<JsonObject>(corrupt).map_err(|error| error.to_string())
    );
    Ok(())
}

#[test]
fn a_message_past_the_serde_json_recursion_limit_parses_whole() -> TestResult {
    let depth = 5_000;
    let text = format!(r#"{{"v": {}1{}}}"#, "[".repeat(depth), "]".repeat(depth));
    assert!(serde_json::from_str::<JsonObject>(&text).is_err());
    let parsed = Value::Object(parse_object(text.as_bytes())?);
    let expected = Value::Object(JsonObject::from_iter([(
        "v".to_owned(),
        nested(depth, false, json!(1)),
    )]));
    assert!(values_equal(&parsed, &expected));
    drop_value(parsed);
    drop_value(expected);
    let corrupt = format!(r#"{{"v": {}1"#, "[".repeat(depth));
    assert!(parse_object(corrupt.as_bytes()).is_err());
    let trailing = format!(r#"{{"v": {}1{}}} x"#, "[".repeat(depth), "]".repeat(depth));
    assert!(parse_object(trailing.as_bytes()).is_err());
    Ok(())
}

#[test]
fn a_malformed_deep_message_fails_as_the_recursive_parser_does_where_a_python_reader_could_meet_it() -> TestResult {
    let depth = 5_000;
    let corrupt = format!(r#"{{"v": {}1 2{}}}"#, "[".repeat(depth), "]".repeat(depth));
    let unbounded = crate::large_stack::on_large_stack(
        &[PARSE_STACK],
        || unbounded_error(corrupt.as_bytes()).map(|error| error.to_string()),
        || None,
        None,
    );
    assert!(unbounded.is_some());
    assert_eq!(
        parse_object(corrupt.as_bytes()).err().map(|error| error.to_string()),
        unbounded
    );
    let limited = |text: &str| {
        serde_json::from_str::<JsonObject>(text)
            .err()
            .map(|error| error.to_string())
    };
    let past_python = format!(r#"{{"v": {}1 2{}}}"#, "[".repeat(DEEP), "]".repeat(DEEP));
    assert_eq!(
        parse_object(past_python.as_bytes())
            .err()
            .map(|error| error.to_string()),
        limited(&past_python)
    );
    Ok(())
}

#[test]
fn where_no_parse_thread_runs_a_malformed_deep_message_meets_the_serde_json_recursion_limit() -> TestResult {
    let depth = 5_000;
    let corrupt = format!(r#"{{"v": {}1 2{}}}"#, "[".repeat(depth), "]".repeat(depth));
    let inline = parse_object_on(&[], corrupt.as_bytes()).map_err(|error| error.to_string());
    let limited = serde_json::from_str::<JsonObject>(&corrupt).map_err(|error| error.to_string());
    assert!(inline.is_err());
    assert_eq!(inline, limited);
    let text = format!(r#"{{"v": {}1{}}}"#, "[".repeat(DEEP), "]".repeat(DEEP));
    let parsed = Value::Object(parse_object_on(&[], text.as_bytes())?);
    let expected = Value::Object(JsonObject::from_iter([("v".to_owned(), nested(DEEP, false, json!(1)))]));
    assert!(values_equal(&parsed, &expected));
    drop_value(parsed);
    drop_value(expected);
    Ok(())
}

#[test]
fn values_clone_compare_print_and_drop_like_serde_json() -> TestResult {
    let value = json!({"b": [1, -2, 2.5, "x\"\n", null, true], "a": {}, "c": [], "d": {"e": {"f": false}}});
    let cloned = clone_value(&value);
    assert_eq!(cloned, value);
    assert!(values_equal(&cloned, &value));
    assert_eq!(to_string(&value), serde_json::to_string(&value)?);
    let object = value.as_object().ok_or("not an object")?;
    assert_eq!(&clone_object(object), object);
    assert_eq!(clone_value(&json!(5)), json!(5));
    assert_eq!(to_string(&json!("a")), r#""a""#);
    drop_value(cloned);
    Ok(())
}

#[test]
fn values_equal_compares_like_serde_json() {
    for (left, right) in [
        (json!({"a": 1, "b": 2}), json!({"b": 2, "a": 1})),
        (json!([1, [2]]), json!([1, [2]])),
        (json!(null), json!(null)),
    ] {
        assert!(values_equal(&left, &right), "{left} {right}");
        assert_eq!(left, right);
    }
    for (left, right) in [
        (json!([1]), json!([1, 2])),
        (json!({"a": 1}), json!({"a": 1, "b": 2})),
        (json!({"a": 1}), json!({"b": 1})),
        (json!({"a": 1}), json!({"a": 2})),
        (json!([1]), json!(1)),
        (json!(1), json!({})),
        (json!(1), json!(1.0)),
        (json!("1"), json!(1)),
    ] {
        assert!(!values_equal(&left, &right), "{left} {right}");
        assert_ne!(left, right);
    }
}

#[test]
fn values_nested_past_the_native_stack_clone_compare_print_and_drop() -> TestResult {
    for objects in [false, true] {
        let value = nested(DEEP, objects, json!(1));
        let cloned = clone_value(&value);
        assert!(values_equal(&value, &cloned));
        let different = nested(DEEP, objects, json!(2));
        assert!(!values_equal(&value, &different));
        let text = to_string(&cloned);
        let (open, close) = if objects { (r#"{"a":"#, "}") } else { ("[", "]") };
        assert_eq!(text, format!("{}1{}", open.repeat(DEEP), close.repeat(DEEP)));
        drop_value(value);
        drop_value(cloned);
        drop_value(different);
    }
    Ok(())
}

/// Goldens of both SDKs' CLI message readers and of `json.loads` near the C recursion limit.
const MESSAGE_DEPTH: &str = include_str!("../tests/fixtures/cli_reader/message_depth.json");

/// The deepest TypeScript golden replayed: a deeper message needs more stack than the parse thread has in an
/// unoptimized build.
const TYPESCRIPT_REPLAYED: usize = 20_000;

/// The containers the scanner goldens nest after their prefix, past the C recursion limit.
const SCANNER_NEST: usize = 20_000;

const ARRAY_RECURSION: Option<&str> =
    Some("maximum recursion depth exceeded while decoding a JSON array from a unicode string");

const OBJECT_RECURSION: Option<&str> =
    Some("maximum recursion depth exceeded while decoding a JSON object from a unicode string");

/// The reader goldens' deep line: `1` inside `depth` arrays, or objects under the key `a`.
fn deep_line(depth: usize, objects: bool) -> String {
    let (open, close) = if objects { (r#"{"a":"#, "}") } else { ("[", "]") };
    format!(
        r#"{{"type":"system","subtype":"deep","v":{}1{}}}"#,
        open.repeat(depth),
        close.repeat(depth)
    )
}

/// How many arrays, or objects under the key `a`, nest around the message's `v`.
fn nesting(message: &JsonObject) -> usize {
    let mut current = message.get("v");
    let mut depth = 0;
    while let Some(inner) = current.and_then(|value| match value {
        Value::Array(items) => items.first(),
        Value::Object(object) => object.get("a"),
        _ => None,
    }) {
        current = Some(inner);
        depth += 1;
    }
    depth
}

fn cases<'a>(fixture: &'a Value, key: &str) -> Result<&'a Vec<Value>, Box<dyn std::error::Error>> {
    fixture
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("no {key} cases").into())
}

#[test]
fn the_python_reader_raises_where_the_sdk_reader_did_and_typescript_reads_any_depth() -> TestResult {
    let fixture: Value = serde_json::from_str(MESSAGE_DEPTH)?;
    for case in cases(&fixture, "reader")? {
        let depth = usize::try_from(case.get("depth").and_then(Value::as_u64).ok_or("no depth")?)?;
        let objects = case.get("kind").and_then(Value::as_str) == Some("object");
        let tail = case.get("tail") == Some(&Value::Bool(true));
        let seen = case.get("seen").and_then(Value::as_array).ok_or("no seen")?;
        let raised = seen.first().and_then(|first| first.get("text")).and_then(Value::as_str);
        let line = deep_line(depth, objects);
        if case.get("sdk").and_then(Value::as_str) == Some("python") {
            let raises = python_raise(line.as_bytes());
            assert_eq!(raises.is_some(), raised.is_some(), "{case}");
            if let Some(expected) = raised {
                assert!(
                    matches!(raises, Some(PythonRaise::Recursion(text)) if text == expected),
                    "{case}"
                );
            }
        } else {
            assert_eq!(raised, None, "{case}");
            let first = seen.first().and_then(Value::as_str);
            assert_eq!(first, Some(format!("system:deep:{depth}").as_str()), "{case}");
        }
        if raised.is_some() {
            assert_eq!(seen.len(), 1, "{case}");
            continue;
        }
        assert_eq!(seen.len(), if tail { 1 } else { 2 }, "{case}");
        if depth > TYPESCRIPT_REPLAYED {
            continue;
        }
        let message = parse_object(line.as_bytes())?;
        assert_eq!(nesting(&message), depth, "{case}");
        drop_value(Value::Object(message));
    }
    Ok(())
}

#[test]
fn json_loads_raises_recursion_only_where_it_reads_the_text_before_the_deep_container() -> TestResult {
    let fixture: Value = serde_json::from_str(MESSAGE_DEPTH)?;
    for case in cases(&fixture, "scanner")? {
        let prefix = case.get("prefix").and_then(Value::as_str).ok_or("no prefix")?;
        let nest = case.get("nest").and_then(Value::as_str).ok_or("no nest")?;
        let raised = case.get("raised").and_then(Value::as_str);
        let text = case.get("text").and_then(Value::as_str);
        let line = format!("{prefix}{}", nest.repeat(SCANNER_NEST));
        let expected = match (raised, text) {
            (Some("RecursionError"), Some(text)) => [ARRAY_RECURSION, OBJECT_RECURSION]
                .into_iter()
                .flatten()
                .find(|known| *known == text)
                .map(PythonRaise::Recursion),
            (Some("ValueError"), Some(text)) => {
                let raise = python_raise(line.as_bytes());
                let Some(PythonRaise::IntDigits(digits)) = raise else {
                    return Err(format!("{prefix:?} {nest:?}: {raise:?}").into());
                };
                assert_eq!(crate::python_re::int_parse_limit_text(digits), text);
                Some(PythonRaise::IntDigits(digits))
            }
            _ => None,
        };
        assert_eq!(expected.is_some(), text.is_some(), "{case}");
        assert_eq!(python_raise(line.as_bytes()), expected, "{prefix:?} {nest:?}");
    }
    Ok(())
}

#[test]
fn the_python_reader_raises_at_the_first_container_past_its_limit() {
    let array = ARRAY_RECURSION.map(PythonRaise::Recursion);
    let object = OBJECT_RECURSION.map(PythonRaise::Recursion);
    assert_eq!(python_raise_past(br#"{"v": [[1]]}"#, 3), None);
    assert_eq!(python_raise_past(br#"{"v": [[1]]}"#, 2), array);
    assert_eq!(python_raise_past(br#" {"v": [{"a": 1}]}"#, 2), object);
    assert_eq!(python_raise_past(br#"{"v": [1, [2]], "w": [[3]]}"#, 2), array);
    assert_eq!(python_raise_past(br#"{"v": [1, []]}"#, 2), array);
    assert_eq!(python_raise_past(br#"{"v": [], "w": {"x": {}}}"#, 2), object);
    assert_eq!(python_raise_past(br#"{"v": [1 [2]]}"#, 2), None);
}

#[test]
fn the_scanner_walk_stops_where_the_text_runs_out() {
    for text in [
        &b"[-"[..],
        br#"["ab"#,
        br#"["\"#,
        br#"["\u1"#,
        br#"{"a"#,
        br#"{"a":1"#,
        b"",
        b"[1]",
        b"{1:2}",
    ] {
        assert_eq!(python_raise_past(text, 9), None, "{}", String::from_utf8_lossy(text));
    }
    let walked = python_raise_past(br#"[ "\u00e9", -0, 1.5E-2, NaN, {"a": []}, [[]]]"#, 2);
    assert_eq!(walked, ARRAY_RECURSION.map(PythonRaise::Recursion));
}

#[test]
fn json_loads_raises_value_error_for_an_integer_past_4300_digits() {
    let digits = |count: usize, sign: &str, tail: &str| format!("[{sign}{}{tail}]", "1".repeat(count));
    assert_eq!(python_raise(digits(4300, "-", "").as_bytes()), None);
    assert_eq!(
        python_raise(digits(4301, "-", "").as_bytes()),
        Some(PythonRaise::IntDigits(4301))
    );
    assert_eq!(python_raise(digits(4301, "", ".5").as_bytes()), None);
    assert_eq!(python_raise(digits(4301, "", "e5").as_bytes()), None);
    assert_eq!(
        python_raise(digits(4301, "", "e").as_bytes()),
        Some(PythonRaise::IntDigits(4301))
    );
    // The deep container comes first, so recursion is what raises.
    let deep = format!("{}{}", "[".repeat(PYTHON_READER_DEPTH + 1), digits(4301, "", ""));
    assert_eq!(
        python_raise(deep.as_bytes()),
        ARRAY_RECURSION.map(PythonRaise::Recursion)
    );
}

#[test]
fn python_strips_the_whitespace_str_strip_takes() {
    let stripped = |text: &str| String::from_utf8_lossy(python_strip(text.as_bytes())).into_owned();
    let spaced =
        "\t\u{b}\u{c}\r\n\u{1c}\u{1f} \u{85}\u{a0}\u{1680}\u{2000}\u{200a}{}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
    assert_eq!(stripped(spaced), "{}");
    for kept in ["\u{0}{}", "\u{180e}{}", "\u{200b}{}", "\u{feff}{}", "{}\u{200b}"] {
        assert_eq!(stripped(kept), kept);
    }
    assert_eq!(python_strip(b" \xff "), b" \xff ");
}

#[test]
fn python_reads_an_integer_negative_zero_as_zero() -> TestResult {
    let line = br#"{"a":-0,"b":[-0],"c":-0.0,"d":-0e1,"e":"-0","f":"\"-0","g":-1,"h":-01}"#;
    let spelled = python_integer_zero(line);
    assert_eq!(spelled.len(), line.len());
    assert_eq!(
        String::from_utf8_lossy(&spelled),
        r#"{"a": 0,"b":[ 0],"c":-0.0,"d":-0e1,"e":"-0","f":"\"-0","g":-1,"h": 01}"#
    );
    let object = parse_object(&python_integer_zero(br#"{"a":-0,"b":-0.0}"#))?;
    assert_eq!(object.get("a").and_then(Value::as_u64), Some(0));
    assert_eq!(
        object.get("b").and_then(Value::as_f64).map(f64::is_sign_negative),
        Some(true)
    );
    assert_eq!(
        String::from_utf8_lossy(&python_integer_zero(br#"{"a":"-0"#)),
        r#"{"a":"-0"#
    );
    assert_eq!(String::from_utf8_lossy(&python_integer_zero(b"[-]")), "[-]");
    Ok(())
}

#[test]
fn typescript_orders_array_index_keys_first_at_any_depth() -> TestResult {
    let mut object: JsonObject = serde_json::from_str(
        r#"{"b":1,"4294967294":2,"4294967295":3,"01":4,"-1":5,"1.5":6,"0":7,"10":8,"9":9,"a":10,
            "x":{"2":1,"1":2},"y":[{"b":1,"0":2}],"z":{"1e3":1,"1000":2," 1":3,"1 ":4,"+1":5,"":6}}"#,
    )?;
    javascript_object(&mut object);
    assert_eq!(
        serde_json::to_string(&object)?,
        r#"{"0":7,"9":9,"10":8,"4294967294":2,"b":1,"4294967295":3,"01":4,"-1":5,"1.5":6,"a":10,"x":{"1":2,"2":1},"y":[{"0":2,"b":1}],"z":{"1000":2,"1e3":1," 1":3,"1 ":4,"+1":5,"":6}}"#
    );
    Ok(())
}

#[test]
fn typescript_reads_each_number_as_the_nearest_double() -> TestResult {
    let mut object: JsonObject = serde_json::from_str(
        r#"{"a":[9007199254740992,9007199254740993,-9007199254740993,18446744073709551615,-9223372036854775808,
            123456789012345678901,-0,0.1,1]}"#,
    )?;
    javascript_object(&mut object);
    assert_eq!(
        serde_json::to_string(&object)?,
        r#"{"a":[9007199254740992,9007199254740992,-9007199254740992,1.8446744073709552e+19,-9223372036854775808,1.2345678901234568e+20,-0.0,0.1,1]}"#
    );
    Ok(())
}

#[test]
fn typescript_reads_type_from_a_null_line() {
    assert!(javascript_null(b"null"));
    assert!(javascript_null(b" \t\r null \n"));
    assert!(!javascript_null(b"\x0cnull"));
    assert!(!javascript_null(b"nul"));
    assert!(!javascript_null(b""));
    assert!(!javascript_null(b"[null]"));
}

#[test]
fn a_message_sets_aside_only_containers_nested_past_the_skeleton_with_a_marker_no_string_starts_with() -> TestResult {
    let shallow = JsonObject::from_iter([("a".to_owned(), nested(SHALLOW - 1, false, json!("leaf")))]);
    assert!(detach(&shallow).is_none());
    let object = JsonObject::from_iter([
        ("\u{0}detached:key".to_owned(), json!("\u{0}detached:#0")),
        ("arrays".to_owned(), nested(SHALLOW + 2, false, json!("leaf"))),
        ("objects".to_owned(), json!([nested(SHALLOW, true, json!(1))])),
    ]);
    let detached = detach(&object).ok_or("nothing was set aside")?;
    assert_eq!(detached.marker, "\u{0}detached:##");
    assert_eq!(detached.subtrees.len(), 2);
    assert_eq!(
        to_string(&nested(SHALLOW - 1, false, json!("\u{0}detached:##0"))),
        to_string(&detached.skeleton["arrays"])
    );
    let mut rebuilt = Value::Object(detached.skeleton.clone());
    detached.reattach(&mut rebuilt);
    assert!(values_equal(&rebuilt, &Value::Object(object.clone())));
    for text in ["\u{0}detached:##x", "\u{0}detached:##2", "\u{0}detached:#0"] {
        let mut unmatched = json!(text);
        detached.reattach(&mut unmatched);
        assert_eq!(unmatched, json!(text));
    }
    Ok(())
}

#[test]
fn errors_that_carry_a_message_nested_past_the_native_stack_drop_it_in_a_loop() {
    let object = || JsonObject::from_iter([("a".to_owned(), nested(DEEP, true, Value::Null))]);
    drop(crate::MessageParseError::new(
        crate::ProtocolError::InvalidMessage("deep".to_owned()),
        Some(object()),
    ));
    drop(crate::ResultError::new("deep".to_owned(), object(), None));
    let mut cleared = object();
    clear_object(&mut cleared);
    assert!(cleared.is_empty());
}

/// The stack the payload checks run on, a sixteenth of a spawned thread's default: a recursive drop of a value
/// nested [`PYTHON_READER_DEPTH`] levels deep overflows it.
const SMALL_STACK: usize = 128 << 10;

/// Runs `check` on a new thread whose stack is [`SMALL_STACK`], failing when it does not hold.
fn on_small_stack(check: impl FnOnce() -> bool + Send + 'static) -> TestResult {
    let held = std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(check)?
        .join()
        .map_err(|_| "the check overflowed or panicked")?;
    if held {
        Ok(())
    } else {
        Err("the check did not hold".into())
    }
}

/// A message whose `v` nests [`PYTHON_READER_DEPTH`] arrays, or objects under the key `a`: as deep as the Python
/// reader reads.
fn reader_deep(objects: bool) -> JsonObject {
    JsonObject::from_iter([("v".to_owned(), nested(PYTHON_READER_DEPTH, objects, Value::Null))])
}

#[test]
fn an_unknown_message_as_deep_as_the_python_reader_reads_clones_destructures_and_drops_in_a_loop() -> TestResult {
    for objects in [false, true] {
        on_small_stack(move || {
            let message = Message::Unknown {
                kind: "deep".to_owned(),
                data: reader_deep(objects).into(),
                raw: reader_deep(objects).into(),
            };
            let copy = message.clone();
            drop(message);
            let Message::Unknown { kind, data, raw } = copy else {
                return false;
            };
            kind == "deep" && nesting(&data) == PYTHON_READER_DEPTH && nesting(&raw) == PYTHON_READER_DEPTH
        })?;
    }
    Ok(())
}

#[test]
fn content_blocks_as_deep_as_the_python_reader_reads_clone_destructure_and_drop_in_a_loop() -> TestResult {
    for objects in [false, true] {
        on_small_stack(move || {
            let blocks = vec![
                ContentBlock::ToolUse {
                    id: ToolUseId::from("tool"),
                    name: ToolName::from("Read"),
                    input: reader_deep(objects).into(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: ToolUseId::from("tool"),
                    content: Value::Object(reader_deep(objects)).into(),
                    status: ToolResultStatus::Success,
                },
                ContentBlock::ServerToolUse {
                    id: ToolUseId::from("server"),
                    name: "advisor".to_owned(),
                    input: reader_deep(objects).into(),
                },
                ContentBlock::ServerToolResult {
                    tool_use_id: ToolUseId::from("server"),
                    content: Value::Object(reader_deep(objects)).into(),
                },
                ContentBlock::Unknown {
                    kind: "deep".to_owned(),
                    data: reader_deep(objects).into(),
                },
            ];
            let copies = blocks.clone();
            drop(blocks);
            copies.into_iter().all(|block| {
                let depth = match block {
                    ContentBlock::ToolUse { input, .. }
                    | ContentBlock::ServerToolUse { input, .. }
                    | ContentBlock::Unknown { data: input, .. } => nesting(&input),
                    ContentBlock::ToolResult { content, .. } | ContentBlock::ServerToolResult { content, .. } => {
                        content.as_object().map_or(0, nesting)
                    }
                    ContentBlock::Text { .. } | ContentBlock::Thinking { .. } => 0,
                };
                depth == PYTHON_READER_DEPTH
            })
        })?;
    }
    Ok(())
}

#[test]
fn the_holders_read_write_compare_print_and_give_back_the_json_they_hold() {
    let object = JsonObject::from_iter([("a".to_owned(), Value::from(1))]);
    let mut held = DeepJsonObject::from(object.clone());
    held.insert("b".to_owned(), Value::Null);
    let mut expected = object;
    expected.insert("b".to_owned(), Value::Null);
    assert_eq!(held, expected);
    assert_eq!(held, DeepJsonObject::from(expected.clone()));
    assert_eq!(format!("{held:?}"), format!("{expected:?}"));
    assert_eq!(held.clone().into_object(), expected);
    assert_eq!(JsonObject::from(held), expected);

    let mut value = DeepJsonValue::from(Value::from(1));
    *value = Value::from(2);
    assert_eq!(value.as_value(), &Value::from(2));
    assert_eq!(value, DeepJsonValue::from(Value::from(2)));
    assert_eq!(format!("{value:?}"), format!("{:?}", Value::from(2)));
    assert_eq!(value.clone().into_value(), Value::from(2));
    assert_eq!(Value::from(value), Value::from(2));
}

#[test]
fn compact_writes_what_serde_json_writes() -> TestResult {
    let values = [
        json!(null),
        json!(true),
        json!(-12),
        json!(1.5),
        json!("a \"quoted\"\n line \u{1}"),
        json!([]),
        json!({}),
        json!([[], {}, [1, [2, {"b": [3]}]], {"": null}]),
        json!({"z": 1, "a": {"y": [true, false], "x": {}}, "m": [{"k": "v"}, []]}),
    ];
    for value in &values {
        assert_eq!(compact(value)?, serde_json::to_string(value)?);
        assert_eq!(
            DeepJsonValue::from(value.clone()).to_json()?,
            serde_json::to_string(value)?
        );
    }
    Ok(())
}

#[test]
fn equal_matches_value_equality_and_ignores_key_order() {
    let left = json!({"a": [1, {"b": 2, "c": [3]}], "d": null});
    let reordered = json!({"d": null, "a": [1, {"c": [3], "b": 2}]});
    assert!(equal(&left, &reordered));
    assert!(!equal(&left, &json!({"a": [1, {"b": 2, "c": [4]}], "d": null})));
    assert!(!equal(&left, &json!({"a": [1, {"b": 2, "c": [3]}], "e": null})));
    assert!(!equal(&left, &json!({"a": [1, {"b": 2, "c": [3]}]})));
    assert!(!equal(&json!([1, 2]), &json!([1])));
    assert!(!equal(&json!([]), &json!({})));
    assert!(!equal(&json!(1), &json!(1.0)));
    assert!(equal(&json!(1.0), &json!(1.0)));
    let deep = DeepJsonValue::from(left.clone());
    let deep_reordered = DeepJsonValue::from(reordered);
    assert!(deep == deep_reordered);
    assert!(deep == left);
}

#[test]
fn nests_within_counts_the_enclosing_containers() {
    assert!(nests_within(&json!(1), 0));
    assert!(!nests_within(&json!([]), 0));
    assert!(nests_within(&json!([]), 1));
    assert!(nests_within(&nested(SHALLOW, false, json!(1)), SHALLOW));
    assert!(!nests_within(
        &nested(SHALLOW.saturating_add(1), false, json!(1)),
        SHALLOW
    ));
    assert!(!nests_within(&json!([1, {"a": [[]]}]), 3));
    assert!(nests_within(&json!([1, {"a": [[]]}]), 4));
}

#[test]
fn a_deep_json_value_prints_as_serde_json_value_prints() {
    let values = [
        json!(null),
        json!(-1.5),
        json!("a \"quoted\"\n line"),
        json!([]),
        json!({}),
        json!([[], {}, [1, [2, {"b": [3]}]], {"": null}]),
        json!({"z": 1, "a": {"y": [true, false], "x": {}}, "m": [{"k": "v"}, []]}),
        nested(SHALLOW.saturating_add(1), true, json!([1, "b"])),
    ];
    for value in values {
        let held = DeepJsonValue::from(value.clone());
        assert_eq!(format!("{held:?}"), format!("{value:?}"));
        assert_eq!(format!("{held:#?}"), format!("{value:#?}"));
    }
}

#[test]
fn undeclared_members_write_as_serde_json_value_while_shallow_and_as_their_text_beyond() -> TestResult {
    let shallow = nested(SHALLOW, false, json!({"b": [1, "c"]}));
    let deep = nested(SHALLOW.saturating_add(1), true, json!({"b": [1, "c"]}));
    for value in [&shallow, &deep] {
        assert_eq!(
            serde_json::to_string(&ShallowWrite(value))?,
            serde_json::to_string(value)?
        );
        assert_eq!(serde_json::to_value(ShallowWrite(value))?, *value);
    }
    Ok(())
}
