#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests compare Python reprs and validation texts of JSON values."
    )
)]

use super::*;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn object(value: Value) -> Result<JsonObject, serde_json::Error> {
    serde_json::from_value(value)
}

#[test]
fn reads_each_shape_as_the_union_does() -> TestResult {
    for (message, kind) in [
        (json!({"jsonrpc":"2.0","id":1,"method":"ping"}), JsonRpcKind::Request),
        (json!({"jsonrpc":"2.0","method":"ping"}), JsonRpcKind::Notification),
        (json!({"jsonrpc":"2.0","id":1,"result":{}}), JsonRpcKind::Response),
        (
            json!({"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"m"}}),
            JsonRpcKind::Response,
        ),
        (
            json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
            JsonRpcKind::Notification,
        ),
    ] {
        assert_eq!(parse(&object(message.clone())?), Ok(kind), "{message}");
    }
    Ok(())
}

#[test]
fn an_empty_message_fails_every_model() -> TestResult {
    let text = parse(&JsonObject::new()).err().ok_or("accepted")?;
    assert!(text.starts_with(
        "11 validation errors for union[JSONRPCRequest,JSONRPCNotification,JSONRPCResponse,JSONRPCError]\nJSONRPCRequest.jsonrpc\n  Field required [type=missing, input_value={}, input_type=dict]\n"
    ));
    assert!(text.ends_with("JSONRPCError.error\n  Field required [type=missing, input_value={}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.14/v/missing"));
    Ok(())
}

#[test]
fn params_that_are_not_a_dictionary_are_named_per_model() -> TestResult {
    let text = parse(&object(json!({"jsonrpc":"2.0","id":"a","method":"ping","params":[]}))?)
        .err()
        .ok_or("accepted")?;
    assert_eq!(
        text,
        "4 validation errors for union[JSONRPCRequest,JSONRPCNotification,JSONRPCResponse,JSONRPCError]\nJSONRPCRequest.params\n  Input should be a valid dictionary [type=dict_type, input_value=[], input_type=list]\n    For further information visit https://errors.pydantic.dev/2.14/v/dict_type\nJSONRPCNotification.params\n  Input should be a valid dictionary [type=dict_type, input_value=[], input_type=list]\n    For further information visit https://errors.pydantic.dev/2.14/v/dict_type\nJSONRPCResponse.result\n  Field required [type=missing, input_value={'jsonrpc': '2.0', 'id': ...': 'ping', 'params': []}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.14/v/missing\nJSONRPCError.error\n  Field required [type=missing, input_value={'jsonrpc': '2.0', 'id': ...': 'ping', 'params': []}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.14/v/missing"
    );
    Ok(())
}
