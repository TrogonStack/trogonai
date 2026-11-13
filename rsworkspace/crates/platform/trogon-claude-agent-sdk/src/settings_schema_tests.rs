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

/// The pinned description with the given fields added (or replaced) and the given restrictive readings appended,
/// so a policy read runs over a schema built to exercise one shape.
fn schema_with(fields: Value, restrictive: Value) -> SettingsSchema {
    let mut document: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("document parses");
    for (name, node) in fields.as_object().expect("fields") {
        let keys = document["keys"].as_array_mut().expect("keys");
        if !keys.iter().any(|key| key == name.as_str()) {
            keys.push(Value::String(name.clone()));
        }
        document["fields"][name.as_str()] = node.clone();
    }
    document["read"]["policy"]["restrictive"]
        .as_array_mut()
        .expect("restrictive")
        .extend(restrictive.as_array().expect("entries").iter().cloned());
    SettingsSchema::parse(&document.to_string()).expect("schema builds")
}

fn read_policy(schema: &SettingsSchema, document: Value) -> Value {
    Value::Object(schema.parse_policy_document(document.as_object().expect("object")))
}

fn optional(inner: Value) -> Value {
    json!({"type": "optional", "inner": inner})
}

fn strings() -> Value {
    json!({"type": "array", "element": {"type": "string"}})
}

/// A locked block whose `list` member is the given schema, beside a locked `lock` and a plain `name`.
fn probe_block(list: Value) -> SettingsSchema {
    schema_with(
        json!({"probe": {"type": "object", "mode": "strip", "shape": {
            "lock": optional(json!({"type": "boolean"})),
            "name": optional(json!({"type": "string"})),
            "list": list,
        }}}),
        json!([{"path": ["probe", "lock"], "restrictive": true}]),
    )
}

fn malformed_after(pointer: &str, replacement: Value) -> Option<String> {
    let mut document: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("document parses");
    *document["read"].pointer_mut(pointer).expect("pointer") = replacement;
    policy::PolicySpec::from_json(&document["read"]).err()
}

#[test]
fn a_malformed_policy_description_names_the_part_that_cannot_be_read() {
    let cases = [
        ("/policy/restrictive", json!(5), "restrictive"),
        (
            "/policy/restrictive",
            json!([{"path": 5, "restrictive": true}]),
            "restrictive",
        ),
        (
            "/policy/restrictive",
            json!([{"path": [1], "restrictive": true}]),
            "restrictive",
        ),
        ("/permissionBlocks", json!(5), "permissionBlocks"),
        ("/permissionBlocks/0/name", json!(5), "block name"),
        ("/permissionBlocks/0/restrictions", json!(5), "restrictions"),
        ("/permissionBlocks/0/grants", json!("x"), "grants"),
        ("/policy/defaultMode", Value::Null, "defaultMode path"),
        ("/policy/defaultMode/modes", json!(5), "modes"),
        ("/policy/defaultMode/aliases", json!({"manual": 5}), "aliases"),
        ("/policy/defaultMode/inert", json!(5), "inert"),
        ("/policy/defaultMode/floor", json!(5), "floor"),
    ];
    for (pointer, replacement, what) in cases {
        assert_eq!(
            malformed_after(pointer, replacement),
            Some(format!("read.policy.{what}")),
            "{pointer}"
        );
    }
    assert_eq!(
        policy::PolicySpec::from_json(&json!({})).err(),
        Some("read.policy.policy".to_owned())
    );
}

#[test]
fn a_null_lock_reads_as_unset_instead_of_its_restrictive_value() {
    let schema = schema().expect("schema loads");
    assert_eq!(
        read_policy(schema, json!({"strictPluginOnlyCustomization": null})),
        json!({})
    );
    assert_eq!(read_policy(schema, json!({"allowManagedHooksOnly": null})), json!({}));
}

#[test]
fn a_strict_plugin_lock_that_is_a_plain_boolean_falls_back_to_true() {
    let schema = schema_with(json!({"strictPluginOnlyCustomization": {"type": "boolean"}}), json!([]));
    assert_eq!(
        read_policy(&schema, json!({"strictPluginOnlyCustomization": false})),
        json!({"strictPluginOnlyCustomization": false})
    );
    assert_eq!(
        read_policy(&schema, json!({"strictPluginOnlyCustomization": "x"})),
        json!({"strictPluginOnlyCustomization": true})
    );
}

#[test]
fn a_lock_below_a_field_that_is_not_an_object_reads_like_a_plain_field() {
    let schema = schema_with(
        json!({"probe": {"type": "string"}}),
        json!([{"path": ["probe", "lock"], "restrictive": true}]),
    );
    assert_eq!(read_policy(&schema, json!({"probe": "x"})), json!({"probe": "x"}));
    assert_eq!(read_policy(&schema, json!({"probe": 5})), json!({}));
}

fn nested_blocks() -> SettingsSchema {
    let deep = json!({"type": "object", "mode": "strict", "shape": {"deep": optional(json!({"type": "boolean"}))}});
    schema_with(
        json!({"probe": {"type": "object", "mode": "strict", "shape": {
            "lock": optional(json!({"type": "boolean"})),
            "inner": optional(deep),
        }}}),
        json!([
            {"path": ["probe", "lock"], "restrictive": true},
            {"path": ["probe", "inner", "deep"], "restrictive": true},
        ]),
    )
}

#[test]
fn a_locked_block_inside_a_locked_block_is_read_member_by_member() {
    let schema = nested_blocks();
    assert_eq!(
        read_policy(&schema, json!({"probe": {"lock": false, "inner": {"deep": false}}})),
        json!({"probe": {"lock": false, "inner": {"deep": false}}})
    );
    assert_eq!(
        read_policy(&schema, json!({"probe": {"lock": "no", "inner": {"deep": "no"}}})),
        json!({"probe": {"lock": true, "inner": {"deep": true}}})
    );
}

#[test]
fn a_strict_block_with_an_unknown_key_reads_as_its_restrictive_skeleton() {
    let schema = nested_blocks();
    assert_eq!(
        read_policy(&schema, json!({"probe": {"extra": 1}})),
        json!({"probe": {"lock": true, "inner": {"deep": true}}})
    );
}

#[test]
fn a_block_that_cannot_be_read_and_has_no_restrictive_member_is_dropped() {
    let schema = schema_with(
        json!({"ghost": {"type": "object", "mode": "strict", "shape": {"a": optional(json!({"type": "boolean"}))}}}),
        json!([{"path": ["ghost", "missing"], "restrictive": true}]),
    );
    assert_eq!(read_policy(&schema, json!({"ghost": {"zzz": 1}})), json!({}));
}

#[test]
fn a_block_that_fails_its_own_checks_reads_as_its_restrictive_skeleton() {
    let source_is_not_a_string = json!({"check": "custom", "fn": {"source": "(e)=>typeof e.source!==\"string\""}});
    let schema = schema_with(
        json!({"gate": {"type": "object", "mode": "strip", "checks": [source_is_not_a_string], "shape": {
            "lock": optional(json!({"type": "boolean"})),
            "source": optional(json!({"type": "unknown"})),
        }}}),
        json!([{"path": ["gate", "lock"], "restrictive": true}]),
    );
    assert_eq!(
        read_policy(&schema, json!({"gate": {"source": "x"}})),
        json!({"gate": {"lock": true}})
    );
    assert_eq!(
        read_policy(&schema, json!({"gate": {"source": 5}})),
        json!({"gate": {"source": 5}})
    );
}

#[test]
fn a_list_behind_a_pipe_keeps_the_entries_the_pipe_accepts() {
    let trimmed = json!({"probe": {"list": ["a"]}});
    let document = json!({"probe": {"list": ["a", 1]}});
    let failing_first = json!({"type": "pipe", "in": strings(), "out": strings()});
    assert_eq!(read_policy(&probe_block(failing_first), document.clone()), trimmed);
    let failing_second = json!({"type": "pipe", "in": {"type": "unknown"}, "out": strings()});
    assert_eq!(read_policy(&probe_block(failing_second), document), trimmed);
}

#[test]
fn a_list_whose_failing_entries_cannot_be_located_is_dropped() {
    let document = json!({"probe": {"name": "n", "list": [1]}});
    let dropped = json!({"probe": {"name": "n"}});
    let stripped_input = json!({"type": "catch", "inner": strings(), "value": {}});
    let unset_middle = json!({"type": "pipe", "in": stripped_input, "out": strings()});
    assert_eq!(read_policy(&probe_block(unset_middle), document.clone()), dropped);
    let text_input = json!({"type": "catch", "inner": strings(), "value": {"value": "x"}});
    let text_middle = json!({"type": "pipe", "in": text_input, "out": strings()});
    assert_eq!(read_policy(&probe_block(text_middle), document.clone()), dropped);
    let union = json!({"type": "union", "options": [strings()]});
    assert_eq!(read_policy(&probe_block(union), document), dropped);
}
