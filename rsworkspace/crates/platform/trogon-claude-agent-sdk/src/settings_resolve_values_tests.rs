#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;

use super::{SettingsMap, merge_with_union, set_path};
use crate::{constants::SETTINGS_SCHEMA_JSON, settings_json::js_stringify};

fn object(value: &Value) -> SettingsMap {
    value.as_object().expect("object").clone()
}

#[test]
fn every_recorded_merge_matches_the_pinned_customizer() {
    let description: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("description");
    let merges = description["merge"]["merges"].as_array().expect("merges");
    assert!(!merges.is_empty());
    for probe in merges {
        let mut merged = object(&probe["destination"]);
        merge_with_union(&mut merged, &object(&probe["source"]));
        assert_eq!(
            js_stringify(&Value::Object(merged)),
            js_stringify(&probe["output"]),
            "merging {} into {}",
            probe["source"],
            probe["destination"]
        );
    }
}

#[test]
fn the_extracted_rules_name_every_special_key() {
    let description: Value = serde_json::from_str(SETTINGS_SCHEMA_JSON).expect("description");
    let merge = &description["merge"];
    assert_eq!(merge["replacedKeys"], serde_json::json!(["modelPicker"]));
    assert_eq!(merge["replacedArrayKeys"], serde_json::json!(["fallbackModel"]));
    assert_eq!(
        merge["shallowObjectKeys"],
        serde_json::json!(["extraKnownMarketplaces", "managedMcpServers"])
    );
}

#[test]
fn setting_at_an_empty_path_leaves_the_settings_alone() {
    let before = object(&serde_json::json!({"model": "a", "permissions": {"allow": ["x"]}}));
    let mut settings = before.clone();
    set_path(&mut settings, &[], Some(&Value::Bool(true)));
    set_path(&mut settings, &[], None);
    assert_eq!(settings, before);
}
