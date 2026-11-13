#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;

use super::{read_document, read_policy_document, sanitize_policy_document};
use crate::{constants::SETTINGS_SCHEMA_JSON, settings_dialect::SettingsDialect, settings_json::js_stringify};

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
        let actual = read_document(input, SettingsDialect::TypeScript).map_or(Value::Null, Value::Object);
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
    // One probe per `NUMBER_DOCUMENTS` entry of `settings_number_battery.mjs`: 126 tokens and 12 container documents.
    assert_eq!(probes.len(), 138);
    // A number past the double range reads as Infinity upstream, which `serde_json` cannot hold, so the document
    // does not load here.
    let infinite = ["1e309", "1e400", "-1e400", "1.7976931348623159e308"];
    let mut mismatches = Vec::new();
    for probe in probes {
        let input = probe["input"].as_str().expect("input");
        let expected = probe["output"].as_str().expect("output");
        let document = read_document(input, SettingsDialect::TypeScript).map(Value::Object);
        let actual = document.as_ref().map_or_else(|| "null".to_owned(), js_stringify);
        if actual == expected {
            continue;
        }
        let token = input
            .strip_prefix("{\"n\":")
            .and_then(|rest| rest.strip_suffix('}'))
            .unwrap_or_default();
        if infinite.contains(&token) {
            continue;
        }
        mismatches.push(format!("{input}\n  rust:     {actual}\n  upstream: {expected}"));
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
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
        let actual = sanitize_policy_document(input).map_or(Value::Null, Value::Object);
        if !same(&actual, expected) {
            mismatches.push(format!("{input}\n  rust:     {actual}\n  upstream: {expected}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn every_recorded_policy_document_is_read_like_the_pinned_resolver() {
    let description: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("description");
    let documents = description["read"]["policy"]["documents"]
        .as_array()
        .expect("documents");
    assert!(documents.len() > 1000);
    let mut mismatches = Vec::new();
    for probe in documents {
        let input = probe["input"].as_str().expect("input");
        let expected = &probe["output"];
        let read = read_policy_document(input);
        let actual = serde_json::json!({
            "settings": read.settings.map_or(Value::Null, Value::Object),
            "documentHasPolicyContent": read.has_policy_content.map_or(Value::Null, Value::Bool),
            "loadState": if read.loaded { "loaded" } else { "didNotLoad" },
        });
        if !same(&actual, expected) {
            mismatches.push(format!("{input}\n  rust:     {actual}\n  upstream: {expected}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn a_blank_managed_document_loads_as_no_settings() {
    for text in ["", " \n\t "] {
        let read = read_policy_document(text);
        assert!(read.settings.expect("settings").is_empty(), "{text:?}");
        assert_eq!(read.has_policy_content, None, "{text:?}");
        assert!(read.loaded, "{text:?}");
    }
}

#[test]
fn a_managed_document_that_is_not_an_object_does_not_load() {
    for text in ["not json", "[1]", "5"] {
        let read = read_policy_document(text);
        assert!(read.settings.is_none(), "{text:?}");
        assert_eq!(read.has_policy_content, None, "{text:?}");
        assert!(!read.loaded, "{text:?}");
    }
}
