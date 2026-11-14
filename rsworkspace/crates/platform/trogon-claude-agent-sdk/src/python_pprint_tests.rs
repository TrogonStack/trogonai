#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests compare Python pprint texts of JSON values.")
)]

use super::*;
use crate::constants::WORDS;
use crate::{python_json_load::drop_python_deep, python_json_value::PythonJsonObject, python_string::PythonString};
use serde_json::{Value as Json, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_pprint.json");

fn segments(value: &Json) -> Vec<PathSegment> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(
                    |item| match item.as_u64().and_then(|index| usize::try_from(index).ok()) {
                        Some(index) => PathSegment::Index(index),
                        None => PathSegment::Key(item.as_str().unwrap_or_default().into()),
                    },
                )
                .collect()
        })
        .unwrap_or_default()
}

fn text<'a>(entry: &'a Json, field: &str) -> &'a str {
    entry.get(field).and_then(Json::as_str).unwrap_or_default()
}

fn py(value: Json) -> Value {
    Value::from(value)
}

#[test]
fn pformat_matches_python_for_every_golden_value() -> TestResult {
    let fixture: Json = serde_json::from_str(FIXTURE)?;
    let entries = fixture.get("pformat").and_then(Json::as_array).ok_or("pformat")?;
    assert!(entries.len() > 500);
    for entry in entries {
        let value = entry.get("value").ok_or("value")?;
        let width = usize::try_from(entry.get("width").and_then(Json::as_u64).ok_or("width")?)?;
        let python = Value::from(value);
        assert_eq!(
            pformat(&python, width, true),
            text(entry, "sorted"),
            "sorted {value} at {width}"
        );
        assert_eq!(
            pformat(&python, width, false),
            text(entry, "unsorted"),
            "unsorted {value} at {width}"
        );
    }
    Ok(())
}

#[test]
fn schema_error_text_matches_python_for_every_golden_error() -> TestResult {
    let fixture: Json = serde_json::from_str(FIXTURE)?;
    let entries = fixture.get("errors").and_then(Json::as_array).ok_or("errors")?;
    assert!(entries.len() > 80);
    for entry in entries {
        let actual = schema_error_text(
            text(entry, "message"),
            Some(text(entry, "validator")),
            &segments(entry.get("schema_path").ok_or("schema_path")?),
            text(entry, "schema_word"),
            &Value::from(entry.get("schema").ok_or("schema")?),
            &segments(entry.get("path").ok_or("path")?),
            text(entry, "instance_word"),
            &Value::from(entry.get("instance").ok_or("instance")?),
        );
        assert_eq!(actual.text, text(entry, "text"), "error {entry}");
    }
    Ok(())
}

#[test]
fn small_values_print_on_one_line() {
    assert_eq!(
        pformat(&py(json!({"b": [1, null, true], "a": 1.5})), 80, true),
        "{'a': 1.5, 'b': [1, None, True]}"
    );
    assert_eq!(pformat(&py(json!({"b": 1, "a": 2})), 80, false), "{'b': 1, 'a': 2}");
    assert_eq!(pformat(&py(json!({})), 1, true), "{}");
    assert_eq!(pformat(&py(json!([])), 1, true), "[]");
    assert_eq!(pformat(&py(json!("")), 1, true), "''");
}

#[test]
fn long_strings_wrap_into_parenthesised_chunks_at_the_top_level_only() {
    let words = "alpha beta gamma delta ".repeat(5);
    let top = pformat(&Value::from(words.as_str()), 40, true);
    assert!(top.starts_with("('alpha beta gamma delta alpha"));
    assert!(top.ends_with("')"));
    let nested = pformat(&py(json!([words])), 40, true);
    assert!(nested.starts_with("['alpha beta gamma delta alpha"));
    assert!(!nested.contains('('));
}

#[test]
fn path_segments_print_as_python_indices() {
    assert_eq!(format_as_index("schema", &[]), "schema");
    assert_eq!(
        format_as_index("schema", &[PathSegment::Key("it's".into()), PathSegment::Index(3)]),
        "schema[\"it's\"][3]"
    );
    let surrogate = PythonString::from_iter([PythonCodePoint::from('a'), PythonCodePoint::from(0xdc80_u16)]);
    assert_eq!(
        format_as_index("instance", &[PathSegment::Key(surrogate)]),
        "instance['a\\udc80']"
    );
}

/// The `str` of each code-point run `parts` holds; none holds a surrogate.
fn texts(parts: Vec<&[PythonCodePoint]>) -> Vec<String> {
    parts
        .into_iter()
        .map(|part| part.iter().filter_map(|code_point| code_point.as_char()).collect())
        .collect()
}

fn code_points(text: &str) -> Vec<PythonCodePoint> {
    text.chars().map(PythonCodePoint::from).collect()
}

#[test]
fn line_boundaries_follow_str_splitlines() {
    assert_eq!(
        splitlines_keep("a\r\nb\rc\u{85}d"),
        vec!["a\r\n", "b\r", "c\u{85}", "d"]
    );
    assert_eq!(splitlines_keep("a\n"), vec!["a\n"]);
    assert!(splitlines_keep("").is_empty());
    let lines = |text: &str| texts(split_code_point_lines(&code_points(text)));
    assert_eq!(lines("a\r\nb\rc\u{85}d"), vec!["a\r\n", "b\r", "c\u{85}", "d"]);
    assert_eq!(lines("a\r"), vec!["a\r"]);
    assert_eq!(lines("a\n"), vec!["a\n"]);
    assert!(lines("").is_empty());
}

#[test]
fn word_parts_alternate_words_and_spaces() {
    let parts = |text: &str| texts(word_parts(&code_points(text)));
    assert_eq!(parts("  a  bc d\u{3000}"), vec!["  ", "a  ", "bc ", "d\u{3000}"]);
    assert!(parts("").is_empty());
}

#[test]
fn a_lone_surrogate_neither_breaks_a_line_nor_splits_words() {
    let text = [
        PythonCodePoint::from('a'),
        PythonCodePoint::from(0xd800_u16),
        PythonCodePoint::from(' '),
    ];
    assert_eq!(split_code_point_lines(&text), vec![&text[..]]);
    assert_eq!(word_parts(&text), vec![&text[..]]);
}

#[test]
fn dedent_removes_only_the_common_margin() {
    assert_eq!(dedent("    a\n      b\n\n    c").text, "a\n  b\n\nc");
    assert_eq!(dedent("    a\n  \n\tb").text, "    a\n\n\tb");
    assert_eq!(dedent("  a\n  b\n x").text, " a\n b\nx");
}

#[test]
fn dedent_substitutes_the_margin_pattern_only_when_there_is_a_margin() {
    assert_eq!(
        dedent("    a\n      b\n\n    c").pattern(),
        Some("(?m)^    ".to_owned())
    );
    assert_eq!(dedent("  a\n  b\n x").pattern(), Some("(?m)^ ".to_owned()));
    assert_eq!(dedent("    a\n  \n\tb").pattern(), None);
    assert_eq!(dedent("a\n  b").pattern(), None);
}

#[test]
fn the_word_and_margin_patterns_compile_in_the_frames_python_spends() {
    let words = PythonString::from(WORDS);
    assert_eq!(crate::python_re::compiles(&words, WORDS_COMPILE_FRAMES), Ok(true));
    assert!(crate::python_re::compiles(&words, WORDS_COMPILE_FRAMES - 1).is_err());
    let margin = PythonString::from(format!("(?m)^{}", " ".repeat(12)));
    assert_eq!(crate::python_re::compiles(&margin, 8), Ok(true));
    assert!(crate::python_re::compiles(&margin, 7).is_err());
}

#[test]
fn python_space_matches_str_isspace() {
    for c in ['\t', '\n', '\u{1f}', ' ', '\u{a0}', '\u{2003}', '\u{3000}'] {
        assert!(is_python_space(c));
    }
    for c in ['a', '\u{200b}', '\u{e000}'] {
        assert!(!is_python_space(c));
    }
}

fn nest_list(depth: usize, leaf: Value) -> Value {
    (0..depth).fold(leaf, |inner, _| Value::Array(vec![inner]))
}

fn nest_dict(depth: usize, leaf: Value) -> Value {
    (0..depth).fold(leaf, |inner, _| object([("a".to_owned(), inner)]))
}

fn object<const N: usize>(entries: [(String, Value); N]) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (PythonString::from(key), value))
            .collect::<PythonJsonObject>(),
    )
}

#[test]
fn pretty_frames_match_the_frames_python_reaches() -> TestResult {
    // Peaks a profiler measured over `str(SchemaError)` with CPython 3.12, relative to the `__str__` frame.
    let cases = || {
        [
            (nest_list(325, py(json!(1))), 982),
            (nest_list(326, py(json!(1))), 985),
            (nest_list(10, py(json!(1))), 27),
            (nest_list(40, py(json!(1))), 127),
            (nest_dict(325, py(json!(1))), 982),
            (nest_dict(10, py(json!(1))), 27),
            (nest_dict(40, py(json!(1))), 127),
            (py(json!({"type": "number"})), 9),
            (nest_list(30, py(json!("x".repeat(100)))), 97),
            (
                Value::Array(vec![nest_list(50, py(json!(1))), nest_list(3, py(json!(1)))]),
                160,
            ),
            (
                object([
                    ("k".repeat(30), nest_dict(50, py(json!(1)))),
                    ("b".to_owned(), py(json!([1, 2, 3]))),
                ]),
                160,
            ),
        ]
        .into_iter()
        .map(|(value, expected)| (pretty_frames(&value, false).peak, expected))
        .collect::<Vec<_>>()
    };
    let measured = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(cases)?
        .join()
        .map_err(|_| "the pretty printer panicked")?;
    for (got, expected) in measured {
        assert_eq!(got, expected);
    }
    Ok(())
}

#[test]
fn the_first_word_split_compiles_the_pattern_only_when_it_is_cold() -> TestResult {
    // Peaks a profiler measured over `_pretty(value)` with CPython 3.12, relative to the `__str__` frame, with `\S*\s*`
    // evicted from the `re` caches (cold) and then cached (warm); the split is the running peak when the first
    // `re.findall` returns from `_compile`.
    let long = || Value::from("x".repeat(100).as_str());
    let cases = move || {
        [
            (nest_list(324, long()), (985, Some(985)), (979, Some(979))),
            (nest_list(30, long()), (103, Some(103)), (97, Some(97))),
            (long(), (13, Some(13)), (7, Some(7))),
            (object([("a".to_owned(), long())]), (16, Some(16)), (10, Some(10))),
            (
                py(json!(["y".repeat(100), "x".repeat(100)])),
                (16, Some(16)),
                (10, Some(10)),
            ),
            (nest_list(10, py(json!(1))), (27, None), (27, None)),
            (py(json!("x")), (7, None), (7, None)),
            (
                Value::Array(vec![nest_list(50, py(json!(1))), long()]),
                (160, Some(160)),
                (160, Some(160)),
            ),
            (
                object([("k".repeat(30), nest_dict(50, py(json!(1)))), ("b".to_owned(), long())]),
                (160, Some(160)),
                (160, Some(160)),
            ),
            (nest_dict(3, long()), (22, Some(22)), (16, Some(16))),
            (
                Value::Array(vec![long(), nest_list(340, py(json!(1)))]),
                (1030, Some(689)),
                (1030, Some(689)),
            ),
        ]
        .into_iter()
        .map(|(value, (cold_peak, cold_split), (warm_peak, warm_split))| {
            (
                pretty_frames(&value, true),
                Reach {
                    peak: cold_peak,
                    split: cold_split,
                },
                pretty_frames(&value, false),
                Reach {
                    peak: warm_peak,
                    split: warm_split,
                },
            )
        })
        .collect::<Vec<_>>()
    };
    let measured = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(cases)?
        .join()
        .map_err(|_| "the pretty printer panicked")?;
    for (cold, cold_expected, warm, warm_expected) in measured {
        assert_eq!(cold, cold_expected);
        assert_eq!(warm, warm_expected);
    }
    Ok(())
}

#[test]
fn values_nested_past_the_native_stack_print_without_recursing() {
    let depth = 5000;
    let value = nest_list(depth, py(json!(1)));
    assert_eq!(
        pformat(&value, 80, true),
        format!("{}1{}", "[".repeat(depth), "]".repeat(depth))
    );
    assert_eq!(
        pretty_frames(&value, false),
        Reach {
            peak: 3 * depth + 7,
            split: None
        }
    );
    drop_python_deep(value);
}
