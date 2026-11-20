use serde_json::{Map, Value, json};

use super::merge_settings;
use crate::{
    constants::V8_MAX_STRING_LENGTH, js_array::LengthError, js_array_mark::ArrayMark, settings_merge_spec::MergeSpec,
    settings_resolve_error::ResolveSettingsError,
};

fn rules() -> MergeSpec {
    MergeSpec {
        replaced: vec!["modelPicker".to_owned()],
        replaced_arrays: vec!["fallbackModel".to_owned()],
        shallow_objects: vec!["managedMcpServers".to_owned()],
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(entries) => entries,
        _ => Map::new(),
    }
}

fn merged(destination: Value, source: Value) -> Result<Value, ResolveSettingsError> {
    let mut destination = object(destination);
    merge_settings(&rules(), &mut destination, &object(source))?;
    Ok(Value::Object(destination))
}

#[test]
fn a_merge_writes_every_member_as_the_settings_customizer_says() {
    let merged = merged(
        json!({
            "list": [1, 2],
            "fallbackModel": ["a"],
            "managedMcpServers": {"one": {"keep": true}},
            "modelPicker": {"old": 1},
            "__proto__": {"a": 1},
            "nested": {"a": 1, "b": 2},
        }),
        json!({
            "list": [2, 3],
            "fallbackModel": ["b"],
            "managedMcpServers": {"one": {"new": true}},
            "modelPicker": {"new": 2},
            "__proto__": {"b": 1},
            "scalar": {"__proto__": 1},
            "nested": {"b": 3, "c": 4},
            "fresh": {"deep": [1, {"x": 1}]},
        }),
    );
    assert_eq!(
        merged.unwrap_or(Value::Null),
        json!({
            "list": [1, 2, 3],
            "fallbackModel": ["b"],
            "managedMcpServers": {"one": {"new": true}},
            "modelPicker": {"new": 2},
            "__proto__": {"a": 1},
            "nested": {"a": 1, "b": 3, "c": 4},
            "scalar": {"__proto__": 1},
            "fresh": {"deep": [1, {"x": 1}]},
        })
    );
}

#[test]
fn an_existing_key_keeps_its_position_and_a_new_key_appends() {
    let merged = merged(json!({"b": 1, "a": 1}), json!({"c": 1, "a": 2})).unwrap_or(Value::Null);
    let keys: Vec<&String> = merged
        .as_object()
        .map(|entries| entries.keys().collect())
        .unwrap_or_default();
    assert_eq!(keys, ["b", "a", "c"]);
    assert_eq!(merged["a"], json!(2));
}

#[test]
fn a_merge_walks_the_members_of_an_object_with_array_indices_first() {
    let merged = merged(json!({"o": {}}), json!({"o": {"b": 1, "1": 2, "a": 3, "0": 4}})).unwrap_or(Value::Null);
    let keys: Vec<&String> = merged["o"]
        .as_object()
        .map(|entries| entries.keys().collect())
        .unwrap_or_default();
    assert_eq!(keys, ["0", "1", "b", "a"]);
}

#[test]
fn an_array_or_object_under_proto_merges_nothing_while_a_scalar_there_is_written() {
    assert_eq!(
        merged(
            json!({}),
            json!({"__proto__": [1], "a": {"__proto__": {"b": 1}, "c": 1}})
        )
        .unwrap_or(Value::Null),
        json!({"a": {"c": 1}})
    );
    assert_eq!(
        merged(json!({"a": {}}), json!({"a": {"__proto__": 7}})).unwrap_or(Value::Null),
        json!({"a": {"__proto__": 7}})
    );
}

#[test]
fn an_object_merged_into_an_array_sets_its_elements_length_and_named_properties() {
    let merged = merged(
        json!({"o": [1, {"j": 2}], "p": [1, 2, 3]}),
        json!({"o": {"0": 5, "1": {"k": 1}, "named": {"q": 1}}, "p": {"length": 1}}),
    )
    .unwrap_or(Value::Null);
    assert_eq!(merged["o"][0], json!(5));
    assert_eq!(merged["o"][1], json!({"j": 2, "k": 1}));
    assert_eq!(merged["p"], json!([1]));
    let again = merged_onto(merged, json!({"o": {"named": {"r": 2}}}));
    assert!(again["o"].is_array());
}

fn merged_onto(destination: Value, source: Value) -> Value {
    merged(destination, source).unwrap_or(Value::Null)
}

#[test]
fn an_object_that_sets_the_length_of_an_array_past_what_it_holds_leaves_holes() {
    let merged = merged_onto(json!({"o": [1]}), json!({"o": {"length": 3}}));
    assert!(merged["o"].is_array());
}

#[test]
fn an_array_merged_onto_an_object_with_a_length_merges_into_a_copy_of_the_object_as_an_array() {
    let holes = Value::Array(vec![json!(9), ArrayMark::FALLBACK.run(2)]);
    let merged = merged_onto(
        json!({"x": {"length": 1, "0": "a"}, "y": {"length": "n"}}),
        json!({"x": holes, "y": [1]}),
    );
    assert!(merged["x"].is_array());
    assert_eq!(merged["y"], json!([1]));
}

#[test]
fn a_length_that_is_not_a_valid_array_length_rejects_with_an_invalid_array_length() {
    assert!(matches!(
        merged(json!({"o": []}), json!({"o": {"length": -1}})),
        Err(ResolveSettingsError::InvalidArrayLength)
    ));
    assert!(matches!(
        merged(json!({"x": {"length": 4294967296_u64}}), json!({"x": [1]})),
        Err(ResolveSettingsError::InvalidArrayLength)
    ));
}

/// Outside a resolve the merge marks the holes of its arrays with [`ArrayMark::FALLBACK`]. The source holds an array
/// of more holes than a V8 string holds commas, as an earlier merge leaves where an object sets the `length` of an
/// array.
#[test]
fn a_length_set_to_an_array_that_joins_past_a_v8_string_rejects_with_an_invalid_string_length() {
    let holes = Value::Array(vec![ArrayMark::FALLBACK.run(V8_MAX_STRING_LENGTH + 2)]);
    let source = json!({"o": {"length": holes}});
    assert!(matches!(
        merged(json!({"o": []}), source),
        Err(ResolveSettingsError::InvalidStringLength)
    ));
}

#[test]
fn a_length_error_is_the_rejection_of_the_same_name() {
    assert!(matches!(
        ResolveSettingsError::from(LengthError::InvalidArrayLength),
        ResolveSettingsError::InvalidArrayLength
    ));
    assert!(matches!(
        ResolveSettingsError::from(LengthError::InvalidStringLength),
        ResolveSettingsError::InvalidStringLength
    ));
}

#[test]
fn a_merge_that_stops_keeps_what_it_wrote_before() {
    let mut destination = object(json!({"o": [], "k": 1}));
    let source = object(json!({"k": 2, "o": {"length": -1}, "z": 3}));
    assert!(merge_settings(&rules(), &mut destination, &source).is_err());
    assert_eq!(Value::Object(destination), json!({"o": [], "k": 2}));
}
