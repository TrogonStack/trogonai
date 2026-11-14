#![allow(clippy::panic)]

use serde_json::{Value, json};

use super::*;

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    }
}

fn python_error(value: Value) -> Option<String> {
    match decode(&object(value), MessageDecoding::Python) {
        Err(ProtocolError::InvalidMessage(message)) => Some(message),
        _ => None,
    }
}

#[test]
fn typescript_consumes_keep_alive_and_python_counts_it_without_yielding_it() -> Result<(), ProtocolError> {
    let keep_alive = object(json!({"type":"keep_alive"}));
    assert!(matches!(
        decode(&keep_alive, MessageDecoding::TypeScript)?,
        Decoded::KeepAlive
    ));
    assert!(matches!(
        decode(&keep_alive, MessageDecoding::Python)?,
        Decoded::Unparsed
    ));
    Ok(())
}

#[test]
fn a_type_that_is_not_a_string_is_yielded_by_typescript_and_skipped_by_python() -> Result<(), ProtocolError> {
    let message = object(json!({"type":5,"field":1}));
    let Decoded::Message(decoded) = decode(&message, MessageDecoding::TypeScript)? else {
        panic!("expected an unknown message");
    };
    let Message::Unknown { kind, data, raw } = *decoded else {
        panic!("expected an unknown message");
    };
    assert_eq!(kind, "");
    assert_eq!(data, object(json!({"field":1})));
    assert_eq!(raw, message);
    assert!(matches!(decode(&message, MessageDecoding::Python)?, Decoded::Unparsed));
    Ok(())
}

#[test]
fn python_reports_a_nested_value_that_is_not_an_object() {
    assert_eq!(
        python_error(json!({"type":"assistant","message":"text"})).as_deref(),
        Some("Field is not an object in assistant message: 'message'")
    );
    assert_eq!(
        python_error(json!({"type":"rate_limit_event","rate_limit_info":[],"uuid":"u","session_id":"s"})).as_deref(),
        Some("Field is not an object in rate_limit_event message: 'rate_limit_info'")
    );
    assert_eq!(
        python_error(
            json!({"type":"result","subtype":"success","duration_ms":1,"duration_api_ms":1,
            "is_error":false,"num_turns":1,"session_id":"s","deferred_tool_use":"tool"})
        )
        .as_deref(),
        Some("Field is not an object in result message: 'deferred_tool_use'")
    );
}

#[test]
fn python_names_the_type_of_a_block_that_is_not_an_object() {
    for (block, name) in [
        (json!(null), "NoneType"),
        (json!(true), "bool"),
        (json!(1.5), "float"),
        (json!("text"), "str"),
        (json!([]), "list"),
    ] {
        assert_eq!(
            python_error(json!({"type":"user","message":{"content":[block]}})),
            Some(format!("Invalid content block (expected dict, got {name})"))
        );
    }
}

#[test]
fn python_checks_hook_messages_before_the_type() -> Result<(), ProtocolError> {
    let message = object(json!({"type":"system","subtype":"hook_response"}));
    let Decoded::Message(decoded) = decode(&message, MessageDecoding::Python)? else {
        panic!("expected a system message");
    };
    let Message::System(system) = *decoded else {
        panic!("expected a system message");
    };
    assert!(matches!(system.event, SystemEvent::Hook(hook) if hook.hook_event_name.is_empty()));
    Ok(())
}

#[test]
fn a_typed_view_needs_the_value_types_python_annotates() {
    let message = object(
        json!({"type":"system","subtype":"task_started","task_id":1,"description":"d",
        "uuid":"u","session_id":"s"}),
    );
    let Some(system) = system(&message) else {
        panic!("expected a system message");
    };
    assert!(matches!(system.event, SystemEvent::Other));
}

#[test]
fn python_truthiness_matches_json_values() {
    for (value, expected) in [
        (None, false),
        (Some(json!(null)), false),
        (Some(json!(false)), false),
        (Some(json!(0)), false),
        (Some(json!(0.0)), false),
        (Some(json!("")), false),
        (Some(json!([])), false),
        (Some(json!({})), false),
        (Some(json!(true)), true),
        (Some(json!(-1)), true),
        (Some(json!("x")), true),
        (Some(json!([0])), true),
        (Some(json!({"a":null})), true),
    ] {
        assert_eq!(truthy(value.as_ref()), expected, "{value:?}");
    }
}
