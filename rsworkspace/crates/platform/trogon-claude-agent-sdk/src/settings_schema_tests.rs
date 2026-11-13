#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};

use super::*;

fn parse(document: Value) -> Option<Map<String, Value>> {
    let schema = schema().expect("schema loads");
    schema.parse_document(document.as_object().expect("object"))
}

#[test]
fn the_schema_description_loads_with_every_runtime_key() {
    let schema = schema().expect("schema loads");
    assert_eq!(schema.keys().len(), 197);
    assert!(schema.keys().iter().any(|key| key == "defaultView"));
}

#[test]
fn an_invalid_value_fails_the_document() {
    assert!(parse(json!({"model": 5})).is_none());
    assert!(parse(json!({"feedbackSurveyRate": 1.5})).is_none());
}

#[test]
fn unknown_keys_pass_through_and_catch_defaults_apply() {
    let parsed = parse(json!({"custom": {"a": 1}, "viewMode": "nope", "model": "m"})).expect("valid");
    assert_eq!(parsed.get("custom"), Some(&json!({"a": 1})));
    assert_eq!(parsed.get("model"), Some(&json!("m")));
    assert!(!parsed.contains_key("viewMode"));
}

#[test]
fn objects_follow_shape_order_and_strip_unknown_keys() {
    let parsed = parse(json!({"breakReminder": {"bogus": 1, "enabled": true}})).expect("valid");
    assert_eq!(parsed["breakReminder"], json!({"enabled": true}));
}

#[test]
fn the_sentinel_never_equals_a_document_value() {
    let sentinel = sentinel();
    assert_ne!(sentinel, &json!({"type": "invalid-entry-stripped"}));
}
