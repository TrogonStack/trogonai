use super::*;
use serde_json::{Value, json};

fn schema() -> Value {
    json!({"file":[{"syntax":"proto3","package":"maps","messageType":[
        {"name":"Entry","options":{"mapEntry":true},"field":[
            {"name":"key","number":1,"type":"TYPE_STRING","label":"LABEL_OPTIONAL"},
            {"name":"value","number":2,"type":"TYPE_STRING","label":"LABEL_OPTIONAL"}
        ]},
        {"name":"Root","field":[
            {"name":"first","number":1,"type":"TYPE_MESSAGE","typeName":".maps.Entry","label":"LABEL_REPEATED"},
            {"name":"second","number":2,"type":"TYPE_MESSAGE","typeName":".maps.Entry","label":"LABEL_REPEATED"},
            {"name":"children","number":3,"type":"TYPE_MESSAGE","typeName":".maps.Root","label":"LABEL_REPEATED"}
        ]}
    ]}]})
}

fn pool(value: &Value) -> WireSchemaPool {
    WireSchemaPool::from_descriptor_set_json(&serde_json::to_vec(value).unwrap()).unwrap()
}

fn field(number: u8, bytes: &[u8]) -> Vec<u8> {
    assert!(bytes.len() < 128);
    [&[(number << 3) | 2, bytes.len() as u8], bytes].concat()
}

fn entry(key: &[u8], value: &[u8]) -> Vec<u8> {
    [field(1, key), field(2, value)].concat()
}

fn check(bytes: &[u8], limits: WireLimits) -> Result<(), WirePreflightError> {
    let pool = pool(&schema());
    preflight_with_string_maps(
        &pool.message_by_name("maps.Root").unwrap(),
        bytes,
        limits,
        &pool.string_map_profile(&["maps.Entry"]).unwrap(),
    )
}

fn limits() -> WireLimits {
    WireLimits::new(4096, 16, 256).unwrap()
}

#[test]
fn profile_requires_exact_schema_arc_and_default_refusal_is_unchanged() {
    let first = pool(&schema());
    let second = pool(&schema());
    let profile = first.string_map_profile(&["maps.Entry"]).unwrap();
    let descriptor = first.clone().message_by_name("maps.Root").unwrap();
    let bytes = field(1, &entry(b"key", b"value"));
    assert_eq!(
        preflight_with_string_maps(&descriptor, &bytes, limits(), &profile),
        Ok(())
    );
    assert_eq!(
        preflight(&descriptor, &bytes, limits()),
        Err(WirePreflightError::UnsupportedMap)
    );
    assert_eq!(
        preflight_with_string_maps(
            &second.message_by_name("maps.Root").unwrap(),
            &bytes,
            limits(),
            &profile
        ),
        Err(WirePreflightError::MapProfileSchema)
    );
}

#[test]
fn profile_rejects_unknown_duplicate_unbounded_and_non_map_names() {
    let pool = pool(&schema());
    for (names, expected) in [
        (vec![], WireStringMapProfileError::EntryCount),
        (vec!["maps.Entry"; 33], WireStringMapProfileError::EntryCount),
        (
            vec!["maps.Entry", "maps.Entry"],
            WireStringMapProfileError::DuplicateEntry,
        ),
        (vec!["missing.Entry"], WireStringMapProfileError::UnknownEntry),
        (vec!["maps.Root"], WireStringMapProfileError::EntryShape),
        (vec![""], WireStringMapProfileError::NameLength),
    ] {
        assert_eq!(pool.string_map_profile(&names).unwrap_err(), expected);
    }
    assert_eq!(
        pool.string_map_profile(&[&"x".repeat(1025)]).unwrap_err(),
        WireStringMapProfileError::NameLength
    );
}

#[test]
fn profile_rejects_every_non_string_singular_optional_entry_shape() {
    for (pointer, value) in [
        ("/file/0/messageType/0/options/mapEntry", json!(false)),
        ("/file/0/messageType/0/field/0/type", json!("TYPE_INT32")),
        ("/file/0/messageType/0/field/1/type", json!("TYPE_BYTES")),
        ("/file/0/messageType/0/field/0/label", json!("LABEL_REPEATED")),
        ("/file/0/messageType/0/field/1/label", json!("LABEL_REQUIRED")),
    ] {
        let mut value_schema = schema();
        *value_schema.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            pool(&value_schema).string_map_profile(&["maps.Entry"]).unwrap_err(),
            WireStringMapProfileError::EntryShape
        );
    }
    for mutation in 0..4 {
        let mut value = schema();
        let map = &mut value["file"][0]["messageType"][0];
        match mutation {
            0 => {
                map["field"].as_array_mut().unwrap().pop();
            }
            1 => {
                map["field"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"name":"extra","number":3,"type":"TYPE_STRING"}));
            }
            2 => {
                map["field"][0]["oneofIndex"] = json!(0);
            }
            _ => {
                map["oneofDecl"] = json!([{"name":"unused"}]);
            }
        }
        assert_eq!(
            pool(&value).string_map_profile(&["maps.Entry"]).unwrap_err(),
            WireStringMapProfileError::EntryShape
        );
    }
}

#[test]
fn map_entries_accept_empty_omitted_and_reordered_original_strings() {
    for bytes in [
        field(1, &[]),
        field(1, &field(1, b"key")),
        field(1, &field(2, b"value")),
        field(1, &entry(b"", b"")),
        field(1, &[field(2, b"value"), field(1, b"key")].concat()),
        [field(1, &entry(b"first", b"one")), field(1, &entry(b"second", b"two"))].concat(),
    ] {
        assert_eq!(check(&bytes, limits()), Ok(()));
    }
}

#[test]
fn map_keys_are_logical_and_scoped_to_each_field_and_message_instance() {
    for entries in [
        [entry(b"same", b"one"), [field(2, b"two"), field(1, b"same")].concat()],
        [vec![], field(1, b"")],
        [field(2, b"one"), entry(b"", b"two")],
    ] {
        assert_eq!(
            check(&[field(1, &entries[0]), field(1, &entries[1])].concat(), limits()),
            Err(WirePreflightError::DuplicateMapKey { field_number: 1 })
        );
    }
    let one = field(1, &entry(b"same", b"one"));
    let second = field(2, &entry(b"same", b"two"));
    assert_eq!(
        check(
            &[one.clone(), second, field(3, &one), field(3, &one)].concat(),
            limits()
        ),
        Ok(())
    );
}

#[test]
fn map_entries_reject_unknown_duplicate_wrong_wire_and_invalid_utf8() {
    for (bytes, expected) in [
        (vec![0x18, 0], WirePreflightError::UnknownField { field_number: 3 }),
        (
            [field(1, b"key"), field(1, b"other")].concat(),
            WirePreflightError::DuplicateSingular { field_number: 1 },
        ),
        (
            [field(2, b"one"), field(2, b"two")].concat(),
            WirePreflightError::DuplicateSingular { field_number: 2 },
        ),
        (vec![0x08, 1], WirePreflightError::WrongWireType { field_number: 1 }),
        (vec![0x10, 1], WirePreflightError::WrongWireType { field_number: 2 }),
        (vec![0x0a, 2, b'x'], WirePreflightError::Truncated),
        (field(1, &[0xff]), WirePreflightError::InvalidUtf8 { field_number: 1 }),
        (field(2, &[0xff]), WirePreflightError::InvalidUtf8 { field_number: 2 }),
    ] {
        assert_eq!(check(&field(1, &bytes), limits()), Err(expected));
    }
}

#[test]
fn map_entries_preserve_global_byte_depth_and_field_budgets() {
    let bytes = field(1, &entry(b"key", b"value"));
    for (limits, expected) in [
        (
            WireLimits::new(bytes.len() - 1, 16, 256).unwrap(),
            WirePreflightError::TooLarge,
        ),
        (WireLimits::new(4096, 1, 256).unwrap(), WirePreflightError::DepthLimit),
        (WireLimits::new(4096, 16, 2).unwrap(), WirePreflightError::FieldLimit),
    ] {
        assert_eq!(check(&bytes, limits), Err(expected));
    }
    assert_eq!(check(&bytes, WireLimits::new(bytes.len(), 2, 3).unwrap()), Ok(()));
}
