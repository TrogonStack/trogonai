use serde_json::{Map, Value, json};

use super::{check_object, check_text, object_depth};
use crate::{constants::SETTINGS_DEPTH_BOUND, settings_resolve_error::ResolveSettingsError};

/// `levels` nested arrays around `null`.
fn arrays(levels: usize) -> Value {
    (0..levels).fold(Value::Null, |value, _| Value::Array(vec![value]))
}

/// `levels` nested objects around `null`.
fn objects(levels: usize) -> Value {
    (0..levels).fold(Value::Null, |value, _| {
        Value::Object(Map::from_iter([("a".to_owned(), value)]))
    })
}

fn nested_text(levels: usize) -> String {
    format!("{}{}", "{\"a\":".repeat(levels - 1) + "{}", "}".repeat(levels - 1))
}

#[test]
fn the_bound_is_the_largest_multiple_of_a_hundred_within_half_the_smallest_recorded_margin() {
    let smallest_margin_depth = 1190;
    let half = smallest_margin_depth / 2;
    assert_eq!(SETTINGS_DEPTH_BOUND, half / 100 * 100);
}

/// The depth of an object holding `value` under `a`, one level deeper than `value` itself.
fn holding(value: Value) -> usize {
    object_depth(&Map::from_iter([("a".to_owned(), value)]))
}

#[test]
fn an_object_of_scalars_is_depth_one_and_each_empty_container_in_it_adds_a_level() {
    assert_eq!(object_depth(&Map::new()), 1);
    assert_eq!(holding(json!(null)), 1);
    assert_eq!(holding(json!("text")), 1);
    assert_eq!(holding(json!({})), 2);
    assert_eq!(holding(json!([])), 2);
}

#[test]
fn each_array_or_object_around_the_deepest_value_adds_a_level() {
    assert_eq!(holding(json!({"a": {}})), 3);
    assert_eq!(holding(json!({"a": 1, "b": [[], [[1]]], "c": {"d": 2}})), 5);
    assert_eq!(holding(json!([1, [2, [3]], {"a": 4}])), 4);
    assert_eq!(holding(arrays(7)), 8);
    assert_eq!(holding(objects(7)), 8);
    assert_eq!(object_depth(&Map::from_iter([("a".to_owned(), json!([[]]))])), 3);
}

#[test]
fn counting_stops_at_the_bound() {
    assert_eq!(holding(arrays(SETTINGS_DEPTH_BOUND - 2)), SETTINGS_DEPTH_BOUND - 1);
    assert_eq!(holding(arrays(SETTINGS_DEPTH_BOUND - 1)), SETTINGS_DEPTH_BOUND);
    assert_eq!(holding(arrays(SETTINGS_DEPTH_BOUND)), SETTINGS_DEPTH_BOUND);
    assert_eq!(holding(objects(4 * SETTINGS_DEPTH_BOUND)), SETTINGS_DEPTH_BOUND);
}

#[test]
fn an_object_document_fails_from_the_bound_and_not_before() {
    let document = |levels: usize| match objects(levels) {
        Value::Object(entries) => entries,
        _ => Map::new(),
    };
    assert!(check_object(&document(SETTINGS_DEPTH_BOUND - 1)).is_ok());
    assert!(matches!(
        check_object(&document(SETTINGS_DEPTH_BOUND)),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
    assert!(matches!(
        check_object(&document(SETTINGS_DEPTH_BOUND + 50)),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
}

#[test]
fn text_that_reads_as_an_object_fails_from_the_bound_and_not_before() {
    assert!(check_text(&nested_text(SETTINGS_DEPTH_BOUND - 1)).is_ok());
    assert!(matches!(
        check_text(&nested_text(SETTINGS_DEPTH_BOUND)),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
    assert!(matches!(
        check_text(&format!("\u{feff}{}", nested_text(SETTINGS_DEPTH_BOUND + 1))),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
}

#[test]
fn text_holding_escapes_of_lone_surrogates_fails_from_the_bound_as_json_parse_reads_it() {
    let holding = |levels: usize, leaf: &str| format!("{}{leaf}{}", "{\"a\":".repeat(levels), "}".repeat(levels));
    let leaf = r#"["\ud800", "\udc00\u0041", "\ud83d\ude00", "\\ud800", "\u00e9\n", "é"]"#;
    assert!(check_text(&holding(SETTINGS_DEPTH_BOUND - 2, leaf)).is_ok());
    assert!(matches!(
        check_text(&holding(SETTINGS_DEPTH_BOUND - 1, leaf)),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
    assert!(matches!(
        check_text(&holding(SETTINGS_DEPTH_BOUND + 20, r#""\udfff""#)),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
    assert!(check_text(&holding(SETTINGS_DEPTH_BOUND, r#""\ud800"#)).is_ok());
}

#[test]
fn text_that_is_no_object_document_passes_at_any_depth() {
    let deep = SETTINGS_DEPTH_BOUND + 10;
    assert!(check_text(&"[".repeat(deep)).is_ok());
    assert!(check_text(&format!("{}{}", "[".repeat(deep), "]".repeat(deep))).is_ok());
    assert!(check_text(&nested_text(deep).replace("{}", "{")).is_ok());
    assert!(check_text("").is_ok());
    assert!(check_text(&format!("\"{}\"", "{".repeat(deep))).is_ok());
}
