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
        repr_json(&json!({"a":[null,true,false,1.5,"é'x"],"b":{}})),
        "{'a': [None, True, False, 1.5, \"é'x\"], 'b': {}}"
    );
    assert_eq!(repr_json(&json!(12345678901234567890_u64)), "12345678901234567890");
    assert_eq!(repr_json(&json!(-3)), "-3");
    assert_eq!(repr_json(&json!(1e20)), "1e+20");
}

#[test]
fn repr_spells_non_finite_floats_and_ints_past_64_bits_as_python_does() -> Result<(), Box<dyn std::error::Error>> {
    let text = "[NaN, Infinity, -Infinity, 1e400, 340282366920938463463374607431768211456]";
    let value = crate::python_json_load::loads_text(text, 2)?;
    assert_eq!(
        repr(&value),
        "[nan, inf, -inf, inf, 340282366920938463463374607431768211456]"
    );
    Ok(())
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
        assert_eq!(json_type_name(&value), name);
        assert_eq!(type_name(&Value::from(&value)), name);
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
        .and_then(serde_json::Value::as_object)
        .ok_or("missing message")?
        .clone();
    scan.into_ids().ok_or("no ids")?.apply_to(&mut message);
    let id = message.get("id").ok_or("missing id")?;
    assert_eq!(repr_json(id), "123456789012345678901234567890");
    assert_eq!(json_type_name(id), "int");
    assert_eq!(type_name(&Value::from(id)), "int");
    assert_eq!(
        repr_json(&serde_json::Value::Object(message)),
        "{'jsonrpc': '2.0', 'id': 123456789012345678901234567890, 'method': 'ping'}"
    );
    Ok(())
}

#[test]
fn repr_nested_sorts_mapping_keys_on_request_and_counts_how_deep_it_nests() {
    assert_eq!(
        repr_nested(&Value::from(json!({"b": 1, "a": []})), true),
        ("{'a': [], 'b': 1}".to_owned(), 2)
    );
    assert_eq!(
        repr_nested(&Value::from(json!({"b": 1, "a": []})), false),
        ("{'b': 1, 'a': []}".to_owned(), 2)
    );
    assert_eq!(repr_nested(&Value::from(json!(1)), false), ("1".to_owned(), 1));
    assert_eq!(repr_nested(&Value::from(json!([])), false), ("[]".to_owned(), 1));
    assert_eq!(repr_nested(&Value::from(json!({})), true), ("{}".to_owned(), 1));
    assert_eq!(
        repr_nested(&Value::from(json!([[1], 2, {"k": [null]}])), false),
        ("[[1], 2, {'k': [None]}]".to_owned(), 4)
    );
}

#[test]
fn repr_writes_values_nested_past_the_native_stack() {
    let depth = 100_000;
    let value = (0..depth).fold(Value::from(1), |inner, _| Value::Array(vec![inner]));
    let (text, deepest) = repr_nested(&value, false);
    assert_eq!(text, format!("{}1{}", "[".repeat(depth), "]".repeat(depth)));
    assert_eq!(deepest, depth + 1);
    drop_python_deep(value);
}
