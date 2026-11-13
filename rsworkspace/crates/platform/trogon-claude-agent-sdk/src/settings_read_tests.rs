#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;

use super::read_document;
use crate::constants::SETTINGS_SCHEMA_JSON;

/// Compares like JavaScript output: key order matters, numbers compare by value.
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
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
        let actual = read_document(input).map_or(Value::Null, Value::Object);
        if !same(&actual, expected) {
            mismatches.push(format!("{input}\n  rust:     {actual}\n  upstream: {expected}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
