#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::json;
use trogon_proto_preflight::{WireLimits, WireSchemaPool, preflight_with_message_maps};

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
