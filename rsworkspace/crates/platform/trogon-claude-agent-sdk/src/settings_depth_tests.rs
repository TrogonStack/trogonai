use serde_json::{Map, Value, json};

use super::{SETTINGS_DEPTH_BOUND, check_object, check_text, depth, object_depth};
use crate::settings_resolve_error::ResolveSettingsError;

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

#[test]
fn a_scalar_is_depth_zero_and_an_empty_container_is_depth_one() {
    assert_eq!(depth(&json!(null)), 0);
    assert_eq!(depth(&json!("text")), 0);
    assert_eq!(depth(&json!({})), 1);
    assert_eq!(depth(&json!([])), 1);
}

#[test]
fn each_array_or_object_around_the_deepest_value_adds_a_level() {
    assert_eq!(depth(&json!({"a": {}})), 2);
    assert_eq!(depth(&json!({"a": 1, "b": [[], [[1]]], "c": {"d": 2}})), 4);
    assert_eq!(depth(&json!([1, [2, [3]], {"a": 4}])), 3);
    assert_eq!(depth(&arrays(7)), 7);
    assert_eq!(depth(&objects(7)), 7);
}

#[test]
fn an_object_is_one_level_deeper_than_the_deepest_of_its_members() {
    let empty = Map::new();
    assert_eq!(object_depth(&empty), 1);
    assert_eq!(object_depth(&Map::from_iter([("a".to_owned(), json!(1))])), 1);
    assert_eq!(object_depth(&Map::from_iter([("a".to_owned(), json!([[]]))])), 3);
}

#[test]
fn counting_stops_at_the_bound() {
    assert_eq!(depth(&arrays(SETTINGS_DEPTH_BOUND - 1)), SETTINGS_DEPTH_BOUND - 1);
    assert_eq!(depth(&arrays(SETTINGS_DEPTH_BOUND)), SETTINGS_DEPTH_BOUND);
    assert_eq!(depth(&arrays(SETTINGS_DEPTH_BOUND + 1)), SETTINGS_DEPTH_BOUND);
    assert_eq!(depth(&objects(4 * SETTINGS_DEPTH_BOUND)), SETTINGS_DEPTH_BOUND);
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
fn text_that_is_no_object_document_passes_at_any_depth() {
    let deep = SETTINGS_DEPTH_BOUND + 10;
    assert!(check_text(&"[".repeat(deep)).is_ok());
    assert!(check_text(&format!("{}{}", "[".repeat(deep), "]".repeat(deep))).is_ok());
    assert!(check_text(&nested_text(deep).replace("{}", "{")).is_ok());
    assert!(check_text("").is_ok());
    assert!(check_text(&format!("\"{}\"", "{".repeat(deep))).is_ok());
}
