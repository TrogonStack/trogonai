#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};

use super::{js_equal, restrict_policy_document};
use crate::{
    python_json_value::PythonNumber,
    settings_json::js_stringify,
    settings_numbers::{PythonNumbers, python_number},
};

#[test]
fn every_recorded_restriction_matches_the_pinned_resolver() {
    let recorded: Value =
        serde_json::from_str(include_str!("upstream/settings_restrict_probes.json")).expect("recorded probes");
    let probes = recorded["probes"].as_array().expect("probes");
    assert!(!probes.is_empty());
    for probe in probes {
        let input = probe["input"].as_object().expect("input document");
        let output = restrict_policy_document(input).map(Value::Object);
        assert_eq!(
            output.as_ref().map(js_stringify),
            probe["output"].as_object().map(|_| js_stringify(&probe["output"])),
            "restricting {}",
            js_stringify(&probe["input"])
        );
    }
}

#[test]
fn the_recorded_probes_cover_kept_and_replaced_documents() {
    let recorded: Value =
        serde_json::from_str(include_str!("upstream/settings_restrict_probes.json")).expect("recorded probes");
    let probes = recorded["probes"].as_array().expect("probes");
    assert!(probes.iter().any(|probe| probe["output"].is_null()));
    assert!(probes.iter().any(|probe| probe["output"].is_object()));
}

#[test]
fn numbers_the_python_dialect_keeps_exact_compare_by_the_double_they_stand_for() {
    let _numbers = PythonNumbers::start();
    let whole = python_number(PythonNumber::Float(1.0));
    let negative_zero = python_number(PythonNumber::Float(-0.0));

    assert!(js_equal(&whole, &json!(1)));
    assert!(js_equal(&json!({"a": [negative_zero]}), &json!({"a": [0]})));
    assert!(!js_equal(&whole, &json!(2)));
}
