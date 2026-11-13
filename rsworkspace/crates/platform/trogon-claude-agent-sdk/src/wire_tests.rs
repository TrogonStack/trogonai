#![allow(clippy::panic)]

use super::*;

#[test]
fn a_known_message_missing_a_required_key_is_yielded_by_typescript_and_rejected_by_python() -> Result<(), ProtocolError>
{
    let frame = br#"{"type":"assistant","message":{"content":[]}}"#;
    assert!(matches!(
        parse_frame(frame)?,
        Frame::Message(Message::Unknown { kind, .. }) if kind == "assistant"
    ));
    let object = serde_json::from_slice(frame)?;
    assert!(matches!(
        parse_value(&object, MessageDecoding::Python),
        Err(ProtocolError::InvalidMessage(message)) if message == "Missing required field in assistant message: 'model'"
    ));
    Ok(())
}

#[test]
fn a_block_rust_cannot_type_stays_in_the_message() -> Result<(), ProtocolError> {
    let frame = br#"{"type":"assistant","message":{"model":"test","content":[{"type":"tool_use","id":"id","name":"Bash","input":null}]}}"#;
    for mode in [MessageDecoding::TypeScript, MessageDecoding::Python] {
        let object = serde_json::from_slice(frame)?;
        let Frame::Message(Message::Assistant(message)) = parse_value(&object, mode)? else {
            panic!("expected an assistant message");
        };
        assert!(
            matches!(&message.content[..], [crate::ContentBlock::Unknown { kind, data }] if kind == "tool_use" && data["input"].is_null())
        );
    }
    Ok(())
}

#[test]
fn future_informational_messages_are_yielded_by_typescript_and_skipped_by_python() -> Result<(), ProtocolError> {
    let frame = br#"{"type":"future_event","field":{"nested":1}}"#;
    assert!(matches!(
        parse_frame(frame)?,
        Frame::Message(Message::Unknown { kind, .. }) if kind == "future_event"
    ));
    let object = serde_json::from_slice(frame)?;
    assert!(matches!(
        parse_value(&object, MessageDecoding::Python)?,
        Frame::Unparsed
    ));
    Ok(())
}

fn permission(bytes: &[u8]) -> Result<ToolPermissionRequest, ProtocolError> {
    let Frame::ControlRequest {
        request: IncomingControl::Permission(request),
        ..
    } = parse_frame(bytes)?
    else {
        panic!("expected a permission request");
    };
    Ok(*request)
}

#[test]
fn broken_permission_requests_reach_the_handler_with_every_field_as_sent() -> Result<(), ProtocolError> {
    for (bytes, raw) in [
        (
            &br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","input":5}}"#[..],
            r#"{"input":5}"#,
        ),
        (
            &br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{},"permission_suggestions":[{"destination":"session"},7]}}"#[..],
            r#"{"tool_name":"Bash","input":{},"permission_suggestions":[{"destination":"session"},7]}"#,
        ),
    ] {
        let request = permission(bytes)?;
        assert_eq!(serde_json::to_string(&request.raw_request)?, raw);
    }
    let request = permission(
        br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","input":5}}"#,
    )?;
    assert_eq!(request.tool_name.as_str(), "");
    assert!(request.input.is_empty());
    assert_eq!(serde_json::to_string(&request.additional_fields)?, r#"{"input":5}"#);
    let request = permission(
        br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{},"permission_suggestions":[{"destination":"session"},7]}}"#,
    )?;
    assert!(matches!(
        request.suggestions.as_slice(),
        [crate::PermissionUpdate::Unknown(_)]
    ));
    Ok(())
}

#[test]
fn permission_fields_of_another_type_stay_in_the_additional_fields() -> Result<(), ProtocolError> {
    let request = permission(
        br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{},"tool_use_id":null,"blocked_path":7,"agent_id":"a","mcp_server":{"name":"n","source":"s"},"default_to_no":"yes","permission_suggestions":"ab"}}"#,
    )?;
    assert!(request.tool_use_id.is_none());
    assert!(request.blocked_path.is_none());
    assert_eq!(request.agent_id.as_deref(), Some("a"));
    assert!(
        matches!(request.mcp_server, Some(crate::McpServerProvenance { ref name, ref source }) if name == "n" && source == "s")
    );
    assert_eq!(request.default_to_no, crate::FeatureSupport::NotReported);
    assert!(request.suggestions.is_empty());
    assert_eq!(
        serde_json::to_string(&request.additional_fields)?,
        r#"{"blocked_path":7,"default_to_no":"yes","permission_suggestions":"ab"}"#
    );
    let request = permission(
        br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{},"mcp_server":{"name":"n"}}}"#,
    )?;
    assert!(request.mcp_server.is_none());
    assert_eq!(
        serde_json::to_string(&request.additional_fields)?,
        r#"{"mcp_server":{"name":"n"}}"#
    );
    Ok(())
}

#[test]
fn unknown_permission_suggestion_values_decode_into_a_permission_request() -> Result<(), ProtocolError> {
    let frame = parse_frame(
        br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{},"permission_suggestions":[{"type":"future"}]}}"#,
    )?;
    assert!(matches!(
        frame,
        Frame::ControlRequest { request: IncomingControl::Permission(request), .. }
            if matches!(request.suggestions.as_slice(), [crate::PermissionUpdate::Unknown(_)])
    ));
    Ok(())
}

#[test]
fn a_failed_handler_answers_its_message_and_a_cancelled_python_handler_writes_nothing() -> Result<(), ProtocolError> {
    let id = ControlRequestId::from("hook");
    let failed = crate::CallbackError::Failed(Box::new(std::io::Error::other("boom")));
    for style in [crate::ArgvStyle::TypeScript, crate::ArgvStyle::Python] {
        let response = handler_error_response(&id, &failed, style)?.map(|frame| frame.to_string());
        assert_eq!(
            response.as_deref(),
            Some(r#"{"type":"control_response","response":{"subtype":"error","request_id":"hook","error":"boom"}}"#)
        );
    }
    assert_eq!(
        handler_error_response(&id, &crate::CallbackError::Cancelled, crate::ArgvStyle::Python)?,
        None
    );
    let aborted = handler_error_response(&id, &crate::CallbackError::Cancelled, crate::ArgvStyle::TypeScript)?
        .map(|frame| frame.to_string());
    assert_eq!(
        aborted.as_deref(),
        Some(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"hook","error":"This operation was aborted"}}"#
        )
    );
    Ok(())
}
