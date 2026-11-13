#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;

use super::{read_document, sanitize_policy_document};
use crate::{
    constants::SETTINGS_SCHEMA_JSON, settings_dialect::SettingsDialect, settings_json::js_stringify,
    settings_numbers::infinities_to_null,
};

/// The document `read_document` yields as the resolver prints it, with each infinity as `null`.
fn printed(text: &str) -> Option<Value> {
    read_document(text, SettingsDialect::TypeScript).map(|document| {
        let mut document = Value::Object(document);
        infinities_to_null(&mut document);
        document
    })
}

/// Compares like JavaScript output: key order matters, and a number must be the same kind (integer or float) with
/// the same value, because the kind decides the text `serde_json` prints.
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left == right,
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(left, right)| same(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|((left_key, left), (right_key, right))| left_key == right_key && same(left, right))
        }
        _ => left == right,
    }
}

#[test]
fn every_recorded_document_reads_like_the_pinned_resolver() {
    let description: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("description");
    let documents = description["read"]["documents"].as_array().expect("documents");
    assert!(documents.len() > 250);
    let mut mismatches = Vec::new();
    for probe in documents {
        let input = probe["input"].as_str().expect("input");
        let expected = &probe["output"];
        let actual = printed(input).unwrap_or(Value::Null);
        if !same(&actual, expected) {
            mismatches.push(format!("{input}\n  rust:     {actual}\n  upstream: {expected}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

fn serialized(document: Option<&Value>) -> String {
    document.map_or_else(|| "null".to_owned(), Value::to_string)
}

#[test]
fn every_recorded_number_document_prints_like_the_pinned_resolver() {
    let description: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("description");
    let probes = description["read"]["numbers"].as_array().expect("numbers");
    // One probe per `NUMBER_DOCUMENTS` entry of `settings_number_battery.mjs`: 126 tokens and 18 container documents.
    assert_eq!(probes.len(), 144);
    let mut mismatches = Vec::new();
    for probe in probes {
        let input = probe["input"].as_str().expect("input");
        let expected = probe["output"].as_str().expect("output");
        let actual = printed(input).as_ref().map_or_else(|| "null".to_owned(), js_stringify);
        if actual == expected {
            continue;
        }
        mismatches.push(format!("{input}\n  rust:     {actual}\n  upstream: {expected}"));
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn a_finite_number_with_the_value_of_an_infinity_marker_prints_like_the_pinned_resolver() {
    // `JSON.stringify(JSON.parse(input))` under Node for the same text.
    let input = r#"{"n":[9223372036854775807.5,1e400],"m":-4611686018427387904.0,"k":9.223372036854775808e18}"#;
    assert_eq!(
        printed(input).as_ref().map(js_stringify).as_deref(),
        Some(r#"{"n":[9223372036854776000,null],"m":-4611686018427388000,"k":9223372036854776000}"#)
    );
}

#[test]
fn the_python_dialect_keeps_large_integers_exact() {
    let document = |text: &str| read_document(text, SettingsDialect::Python).map(Value::Object);
    assert_eq!(
        serialized(document(r#"{"n":12345678901234567890}"#).as_ref()),
        r#"{"n":12345678901234567890}"#
    );
    assert_eq!(
        serialized(document(r#"{"n":9007199254740993,"m":-0}"#).as_ref()),
        r#"{"n":9007199254740993,"m":0}"#
    );
}

#[test]
fn every_recorded_policy_document_is_sanitized_like_the_pinned_resolver() {
    let description: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("description");
    let documents = description["read"]["policySanitized"].as_array().expect("documents");
    assert!(documents.len() > 250);
    let mut mismatches = Vec::new();
    for probe in documents {
        let input = probe["input"].as_str().expect("input");
        let expected = &probe["output"];
        let actual = sanitize_policy_document(input).map_or(Value::Null, |document| {
            let mut document = Value::Object(document);
            infinities_to_null(&mut document);
            document
        });
        if !same(&actual, expected) {
            mismatches.push(format!("{input}\n  rust:     {actual}\n  upstream: {expected}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
