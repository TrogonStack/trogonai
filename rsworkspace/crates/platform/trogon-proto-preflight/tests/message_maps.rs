#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::json;
use trogon_proto_preflight::{WireLimits, WirePreflightError, WireSchemaPool, preflight, preflight_with_message_maps};

fn pool() -> WireSchemaPool {
    let descriptor = json!({"file":[{"syntax":"proto3","package":"example","messageType":[
        {"name":"Entry","options":{"mapEntry":true},"field":[
            {"name":"key","number":1,"type":"TYPE_STRING","label":"LABEL_OPTIONAL"},
            {"name":"value","number":2,"type":"TYPE_MESSAGE","typeName":".example.Value","label":"LABEL_OPTIONAL"}
        ]},
        {"name":"Value","oneofDecl":[{"name":"kind"}],"field":[
            {"name":"text","number":1,"type":"TYPE_STRING","label":"LABEL_OPTIONAL","oneofIndex":0}
        ]},
        {"name":"Root","field":[
            {"name":"entries","number":1,"type":"TYPE_MESSAGE","typeName":".example.Entry","label":"LABEL_REPEATED"}
        ]}
    ]}]});
    WireSchemaPool::from_descriptor_set_json(&serde_json::to_vec(&descriptor).unwrap()).unwrap()
}

#[test]
fn exact_registered_message_map_is_checked_without_discarding_original_values() {
    let schema = pool();
    let profile = schema
        .message_map_profile(&[("example.Entry", "example.Value")])
        .unwrap();
    // Root.entries = { key: "x", value: { text: "y" } }.
    let original = [10, 8, 10, 1, b'x', 18, 3, 10, 1, b'y'];
    assert_eq!(
        preflight_with_message_maps(
            &schema.message_by_name("example.Root").unwrap(),
            &original,
            WireLimits::new(1024, 8, 32).unwrap(),
            &profile,
        ),
        Ok(())
    );
}

#[test]
fn message_map_profile_cannot_widen_other_schemas_or_hide_duplicate_keys() {
    let first = pool();
    let second = pool();
    let profile = first
        .message_map_profile(&[("example.Entry", "example.Value")])
        .unwrap();
    let descriptor = first.message_by_name("example.Root").unwrap();
    let limits = WireLimits::new(1024, 8, 32).unwrap();
    let original = [10, 8, 10, 1, b'x', 18, 3, 10, 1, b'y'];
    assert_eq!(
        preflight(&descriptor, &original, limits),
        Err(WirePreflightError::UnsupportedMap)
    );
    assert_eq!(
        preflight_with_message_maps(
            &second.message_by_name("example.Root").unwrap(),
            &original,
            limits,
            &profile
        ),
        Err(WirePreflightError::MapProfileSchema)
    );
    assert_eq!(
        preflight_with_message_maps(&descriptor, &[original, original].concat(), limits, &profile),
        Err(WirePreflightError::DuplicateMapKey { field_number: 1 })
    );
    let nested_unknown = [10, 8, 10, 1, b'x', 18, 3, 18, 1, b'y'];
    assert_eq!(
        preflight_with_message_maps(&descriptor, &nested_unknown, limits, &profile),
        Err(WirePreflightError::UnknownField { field_number: 2 })
    );
    for entries in [
        vec![],
        vec![("example.Entry", "example.Root")],
        vec![("example.Root", "example.Value")],
        vec![("example.Entry", "example.Value"); 2],
    ] {
        assert!(first.message_map_profile(&entries).is_err());
    }
    for (limits, expected) in [
        (WireLimits::new(1, 8, 32).unwrap(), WirePreflightError::TooLarge),
        (WireLimits::new(1024, 1, 32).unwrap(), WirePreflightError::DepthLimit),
        (WireLimits::new(1024, 8, 1).unwrap(), WirePreflightError::FieldLimit),
    ] {
        assert_eq!(
            preflight_with_message_maps(&descriptor, &original, limits, &profile),
            Err(expected)
        );
    }
}
