#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};

use super::*;
use crate::{constants::SETTINGS_SCHEMA_JSON, settings_schema_builtins::probe_matches};

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

fn batteries(document: &serde_json::Value) -> Batteries {
    let load = |name: &str| -> Vec<serde_json::Value> {
        document["batteries"][name]
            .as_array()
            .map(|items| items.iter().map(super::expand_battery).collect())
            .unwrap_or_default()
    };
    Batteries {
        string: load("string"),
        object: load("object"),
        value: load("value"),
        rule: load("rule"),
    }
}

/// Every function description in the document: `(owner, kind)` where the owner carries `probes`.
fn collect_functions<'a>(node: &'a serde_json::Value, found: &mut Vec<(&'a serde_json::Value, &'a serde_json::Value)>) {
    match node {
        serde_json::Value::Object(object) => {
            if let Some(function) = object.get("fn") {
                found.push((function, node));
            }
            if let Some(payload) = object.get("payloadCheck") {
                found.push((payload, node));
            }
            if object.get("type").and_then(serde_json::Value::as_str) == Some("transform") {
                found.push((node, node));
            }
            object.values().for_each(|child| collect_functions(child, found));
        }
        serde_json::Value::Array(items) => items.iter().for_each(|child| collect_functions(child, found)),
        _ => {}
    }
}

#[test]
fn no_schema_function_is_left_unported() {
    let schema = schema().expect("schema loads");
    let mut unported = schema.unported_sources();
    unported.dedup();
    assert!(unported.is_empty(), "unported: {unported:?}");
}

#[test]
fn every_port_reproduces_the_recorded_probes() {
    let document: serde_json::Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("document parses");
    let batteries = batteries(&document);
    let mut functions = Vec::new();
    collect_functions(&document, &mut functions);
    assert!(functions.len() > 100, "found {} functions", functions.len());
    let mut checked = 0;
    let mut mismatches: Vec<String> = Vec::new();
    for (owner, entry) in functions {
        let source = owner["source"].as_str().expect("source");
        let recorded: Vec<String> = owner["probes"]
            .as_array()
            .expect("probes")
            .iter()
            .map(|probe| probe.as_str().expect("probe text").to_owned())
            .collect();
        if entry.get("payloadCheck").is_some() && std::ptr::eq(owner, &entry["payloadCheck"]) {
            let battery = batteries.named(owner["battery"].as_str().expect("battery"));
            let payload = payload_for(source, owner, &batteries);
            for (input, recorded) in battery.iter().zip(owner["probes"].as_array().expect("probes")) {
                let text = recorded.as_str().expect("probe text");
                if text.starts_with("error:") {
                    continue;
                }
                let count = serde_json::from_str::<Vec<Value>>(text).expect("issue list").len();
                let produced = payload.issues(input);
                if count != produced {
                    mismatches.push(format!(
                        "payload {source} on {input}: recorded {count}, port {produced}"
                    ));
                }
            }
        } else if entry.get("type").and_then(serde_json::Value::as_str) == Some("transform") {
            let transform = transform_for(source, entry, &batteries);
            for (input, expected) in batteries.named("value").iter().zip(&recorded) {
                let produced = transform.apply(Some(input));
                if !probe_matches(expected, produced.as_ref()) {
                    mismatches.push(format!(
                        "transform {source} on {input}: recorded {expected}, port {produced:?}"
                    ));
                }
            }
        } else {
            let battery = batteries.named(owner["battery"].as_str().expect("battery"));
            let predicate = predicate_for(source, entry, &batteries);
            for (input, expected) in battery.iter().zip(&recorded) {
                if expected.starts_with("error:") {
                    continue;
                }
                let produced = if predicate.passes(input) { "true" } else { "false" };
                if *expected != produced {
                    mismatches.push(format!(
                        "predicate {source} on {input}: recorded {expected}, port {produced}"
                    ));
                }
            }
        }
        checked += 1;
    }
    assert!(checked > 100);
    mismatches.dedup();
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(400)
            .map(|line| line.chars().take(260).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
