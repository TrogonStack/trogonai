use std::collections::BTreeSet;

use serde_json::Value;

use super::ModelToolSpec;
use super::constants::{ACKNOWLEDGE, ROSTER, SEND, STATUS};
use crate::model_tool_definitions::model_tool;

#[test]
fn argument_specs_match_the_advertised_input_schemas() {
    for spec in [&ROSTER, &SEND, &STATUS, &ACKNOWLEDGE] {
        let schema = model_tool(spec.name).map(|tool| tool.input_schema);
        let schema = schema.as_deref();
        let properties: BTreeSet<&str> = schema
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object)
            .map(|properties| properties.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let required: BTreeSet<&str> = schema
            .and_then(|schema| schema.get("required"))
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(properties, names(spec, |_| true), "{} fields", spec.name);
        assert_eq!(required, names(spec, |field| field.required), "{} required", spec.name);
    }
}

fn names(spec: &ModelToolSpec, keep: impl Fn(&super::FieldSpec) -> bool) -> BTreeSet<&str> {
    spec.fields
        .iter()
        .filter(|field| keep(field))
        .map(|field| field.name)
        .collect()
}
