#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};
use trogon_proto_preflight::{WireLimits, WirePreflightError, WireSchemaError, WireSchemaPool, preflight};

fn pool(descriptor: &Value) -> Result<WireSchemaPool, WireSchemaError> {
    WireSchemaPool::from_descriptor_set_json(&serde_json::to_vec(descriptor).unwrap())
}

fn schema(field: Value) -> Value {
    json!({"file":[{"syntax":"proto3","package":"example","messageType":[{"name":"Record","field":[field]}]}]})
}

#[test]
fn descriptor_document_bounds_and_supported_syntax_are_enforced_before_use() {
    assert!(matches!(
        WireSchemaPool::from_descriptor_set_json(&vec![b' '; 16 * 1024 * 1024 + 1]),
        Err(WireSchemaError::InvalidDescriptor)
    ));
    assert!(matches!(
        WireSchemaPool::from_descriptor_set_json(b"{"),
        Err(WireSchemaError::Json(_))
    ));
    for descriptor in [json!({}), json!({"file":{}}), json!({"file":[{"messageType":[{}]}]})] {
        assert!(matches!(pool(&descriptor), Err(WireSchemaError::InvalidDescriptor)));
    }
    assert!(matches!(
        pool(&json!({"file":[{"syntax":"proto9"}]})),
        Err(WireSchemaError::UnsupportedSyntax)
    ));
}

#[test]
fn dangling_message_group_and_enum_references_refuse_the_entire_descriptor_pool() {
    for kind in ["TYPE_MESSAGE", "TYPE_GROUP", "TYPE_ENUM"] {
        for type_name in [None, Some(".missing.Target")] {
            let mut field = json!({"name":"reference","number":1,"type":kind});
            if let Some(type_name) = type_name {
                field["typeName"] = json!(type_name);
            }
            assert!(matches!(pool(&schema(field)), Err(WireSchemaError::MissingType)));
        }
    }
}

#[test]
fn ambiguous_type_names_are_refused_across_files_and_nested_scopes() {
    for declaration in [
        json!({"messageType":[{"name":"Record"}]}),
        json!({"enumType":[{"name":"State","value":[{"name":"UNKNOWN","number":0}]}]}),
        json!({"messageType":[{"name":"Record","nestedType":[{"name":"Child"},{"name":"Child"}]}]}),
    ] {
        let mut file = declaration;
        file["package"] = json!("example");
        let descriptor = if file["messageType"][0]["nestedType"].is_array() {
            json!({"file":[file]})
        } else {
            json!({"file":[file.clone(),file]})
        };
        assert!(matches!(pool(&descriptor), Err(WireSchemaError::DuplicateName)));
    }
}

#[test]
fn invalid_field_metadata_and_duplicate_numbers_cannot_define_a_wire_shape() {
    for field in [
        json!({"name":"id","number":1,"type":"TYPE_UUID"}),
        json!({"name":"id","number":1,"type":"TYPE_STRING","label":"LABEL_UNKNOWN"}),
        json!({"name":"id","number":0,"type":"TYPE_STRING"}),
        json!({"name":"id","number":536870912u64,"type":"TYPE_STRING"}),
        json!({"name":"id","number":1}),
    ] {
        assert!(matches!(pool(&schema(field)), Err(WireSchemaError::InvalidDescriptor)));
    }
    let mut descriptor = schema(json!({"name":"id","number":1,"type":"TYPE_STRING"}));
    descriptor["file"][0]["messageType"][0]["field"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"other","number":1,"type":"TYPE_STRING"}));
    assert!(matches!(pool(&descriptor), Err(WireSchemaError::InvalidDescriptor)));
}

#[test]
fn unqualified_nested_types_and_edition_required_presence_survive_descriptor_ingestion() {
    for presence in [json!("LEGACY_REQUIRED"), json!(3)] {
        let descriptor = json!({"file":[{"syntax":"editions","messageType":[{
            "name":"Envelope",
            "enumType":[{"name":"State","value":[{"name":"UNKNOWN","number":0},{"name":"READY","number":1}]}],
            "nestedType":[{"name":"Payload","field":[{"name":"state","number":1,"type":"TYPE_ENUM","typeName":".Envelope.State"}]}],
            "field":[{"name":"payload","number":1,"type":"TYPE_MESSAGE","typeName":".Envelope.Payload","options":{"features":{"fieldPresence":presence}}}]
        }]}]});
        let schema = pool(&descriptor).unwrap();
        let message = schema.message_by_name("Envelope").unwrap();
        assert!(schema.message_by_name("Envelope.Payload").is_some());
        assert!(schema.message_by_name("example.Envelope").is_none());
        let limits = WireLimits::new(256, 8, 16).unwrap();
        assert_eq!(
            preflight(&message, &[], limits),
            Err(WirePreflightError::MissingRequired { field_number: 1 })
        );
        assert_eq!(preflight(&message, &[10, 2, 8, 1], limits), Ok(()));
        assert_eq!(
            preflight(&message, &[10, 2, 8, 2], limits),
            Err(WirePreflightError::UnknownEnum { field_number: 1 })
        );
    }
}
