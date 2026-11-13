#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests compare the exact JSON-RPC shapes each SDK writes."
    )
)]

use super::*;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn object(value: Value) -> Result<JsonObject, serde_json::Error> {
    serde_json::from_value(value)
}

#[test]
fn typescript_waits_only_on_a_method_with_an_id() -> TestResult {
    assert!(typescript_expects_response(&object(json!({"method":"ping","id":0}))?));
    assert!(typescript_expects_response(&object(json!({"method":null,"id":"a"}))?));
    assert!(!typescript_expects_response(&object(
        json!({"method":"ping","id":null})
    )?));
    assert!(!typescript_expects_response(&object(json!({"method":"ping"}))?));
    assert!(!typescript_expects_response(&object(json!({"id":1,"result":{}}))?));
    Ok(())
}

#[test]
fn acknowledgments_keep_each_sdk_key_order() -> TestResult {
    assert_eq!(
        serde_json::to_string(&typescript_acknowledgment())?,
        r#"{"jsonrpc":"2.0","result":{},"id":0}"#
    );
    assert_eq!(
        serde_json::to_string(&python_acknowledgment())?,
        r#"{"jsonrpc":"2.0","result":{}}"#
    );
    Ok(())
}

#[test]
fn python_error_answers_under_the_message_id_or_null() -> TestResult {
    let answered = python_error(&object(json!({"id":"a","method":"ping"}))?, -32603, "boom".into());
    assert_eq!(
        serde_json::to_string(&answered)?,
        r#"{"jsonrpc":"2.0","id":"a","error":{"code":-32603,"message":"boom"}}"#
    );
    let anonymous = python_error(&JsonObject::new(), -32601, "Server 'x' not found".into());
    assert_eq!(
        serde_json::to_string(&anonymous)?,
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32601,"message":"Server 'x' not found"}}"#
    );
    Ok(())
}

#[test]
fn js_string_matches_the_javascript_string_conversion() {
    for (value, expected) in [
        (None, "undefined"),
        (Some(json!(null)), "null"),
        (Some(json!(true)), "true"),
        (Some(json!(false)), "false"),
        (Some(json!(5)), "5"),
        (Some(json!(-3)), "-3"),
        (Some(json!(1.0)), "1"),
        (Some(json!(-2.0)), "-2"),
        (Some(json!(1.5)), "1.5"),
        (Some(json!("name")), "name"),
        (Some(json!([])), ""),
        (Some(json!([1, null, "a", [2, 3], {}])), "1,,a,2,3,[object Object]"),
        (Some(json!({"a":1})), "[object Object]"),
    ] {
        assert_eq!(js_string(value.as_ref()), expected, "{value:?}");
    }
}

#[test]
fn typescript_not_an_object_names_the_value_like_v8() {
    assert_eq!(
        typescript_not_an_object(None),
        "Cannot use 'in' operator to search for 'method' in undefined"
    );
    assert_eq!(
        typescript_not_an_object(Some(&json!("text"))),
        "Cannot use 'in' operator to search for 'method' in text"
    );
    assert_eq!(
        typescript_not_an_object(Some(&json!(7))),
        "Cannot use 'in' operator to search for 'method' in 7"
    );
}
