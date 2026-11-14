#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests compare Python reprs and validation texts of JSON values."
    )
)]

use super::*;
use crate::{JsonObject, mcp_wire_ids::McpWireScan};
use serde_json::json;

#[test]
fn repr_spells_values_as_python_does() {
    assert_eq!(
        repr(&json!({"a":[null,true,false,1.5,"é'x"],"b":{}})),
        "{'a': [None, True, False, 1.5, \"é'x\"], 'b': {}}"
    );
    assert_eq!(repr(&json!(12345678901234567890_u64)), "12345678901234567890");
    assert_eq!(repr(&json!(-3)), "-3");
    assert_eq!(repr(&json!(1e20)), "1e+20");
}

#[test]
fn type_name_is_the_python_class_name() {
    for (value, name) in [
        (json!(null), "NoneType"),
        (json!(true), "bool"),
        (json!(1), "int"),
        (json!(1.0), "float"),
        (json!("a"), "str"),
        (json!([]), "list"),
        (json!({}), "dict"),
    ] {
        assert_eq!(type_name(&value), name);
    }
}

#[test]
fn a_request_id_beyond_64_bits_is_the_integer_python_read() -> Result<(), Box<dyn std::error::Error>> {
    let frame = r#"{"type":"control_request","request_id":"r","request":{"subtype":"mcp_message","server_name":"s","message":{"jsonrpc":"2.0","id":123456789012345678901234567890,"method":"ping"}}}"#;
    let scan = McpWireScan::new(frame.as_bytes());
    let object: JsonObject = serde_json::from_slice(scan.frame(frame.as_bytes()))?;
    let mut message = object
        .get("request")
        .and_then(|request| request.get("message"))
        .and_then(Value::as_object)
        .ok_or("missing message")?
        .clone();
    scan.into_ids().ok_or("no ids")?.apply_to(&mut message);
    let id = message.get("id").ok_or("missing id")?;
    assert_eq!(repr(id), "123456789012345678901234567890");
    assert_eq!(type_name(id), "int");
    assert_eq!(
        repr(&Value::Object(message)),
        "{'jsonrpc': '2.0', 'id': 123456789012345678901234567890, 'method': 'ping'}"
    );
    Ok(())
}
