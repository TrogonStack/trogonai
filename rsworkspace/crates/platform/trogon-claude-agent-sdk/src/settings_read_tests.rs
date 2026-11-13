#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;

use super::read_document;
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

/// The number tokens of a probe document as written: every piece between JSON punctuation that starts like a number.
fn number_tokens(input: &str) -> impl Iterator<Item = &str> {
    input
        .split(['{', '}', '[', ']', ',', ':'])
        .filter(|piece| piece.starts_with(|first: char| first == '-' || first.is_ascii_digit()))
}

#[test]
fn every_recorded_number_document_prints_like_the_pinned_resolver() {
    let description: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("description");
    let probes = description["read"]["numbers"].as_array().expect("numbers");
    // One probe per `NUMBER_DOCUMENTS` entry of `settings_number_battery.mjs`: 126 tokens and 12 container documents.
    assert_eq!(probes.len(), 138);
    // A double of at least 2^64 (below 1e21) or below -2^63, and a fraction between 1e-6 and 1e-5, read exactly but
    // `serde_json` prints them in exponent form where JavaScript prints digits, alone or inside a container. A number
    // past the double range reads as Infinity upstream, which `serde_json` cannot hold, so the document does not load
    // here.
    let printed_differently = [
        "-12345678901234567890",
        "-9223372036854775807",
        "-9223372036854775808",
        "-9223372036854775809",
        "18446744073709550592",
        "18446744073709551614",
        "18446744073709551615",
        "18446744073709551616",
        "18446744073709551617",
        "18446744073709553664",
        "20000000000000000000",
        "123456789012345678901",
        "100000000000000000000",
        "-100000000000000000000",
        "-18446744073709551616",
        "0.000001",
        "1e-6",
        "1.5e-6",
        "9.99e-6",
        "0.0000015",
    ];
    let infinite = ["1e309", "1e400", "-1e400", "1.7976931348623159e308"];
    let mut mismatches = Vec::new();
    for probe in probes {
        let input = probe["input"].as_str().expect("input");
        let expected = probe["output"].as_str().expect("output");
        let document = read_document(input, SettingsDialect::TypeScript).map(Value::Object);
        let actual = serialized(document.as_ref());
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
        let javascript = document.as_ref().map(js_stringify);
        if number_tokens(input).any(|number| printed_differently.contains(&number))
            && javascript.as_deref() == Some(expected)
        {
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
