#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;

use super::{SettingsMap, merge, set_path, union_arrays};
use crate::{
    constants::SETTINGS_SCHEMA_JSON, js_array::settle, settings_json::js_stringify, settings_strings::array_mark,
};

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
        merge(&mut merged, &object(&probe["source"])).expect("merges");
        let mut settled = [Value::Object(merged)];
        settle(&mut settled, array_mark()).expect("settles");
        let [merged] = settled;
        assert_eq!(
            js_stringify(&merged),
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

#[test]
fn a_union_keeps_one_of_each_primitive_by_same_value_zero_and_every_object() {
    let left: Vec<Value> = serde_json::from_str(r#"[1,{"a":1},"x"]"#).unwrap();
    let right: Vec<Value> = serde_json::from_str(r#"[1.0,{"a":1},-0.0,0,"x",[2],[2]]"#).unwrap();
    let expected: Vec<Value> = serde_json::from_str(r#"[1,{"a":1},"x",{"a":1},-0.0,[2],[2]]"#).unwrap();
    assert_eq!(union_arrays(&left, &right), expected);
}
