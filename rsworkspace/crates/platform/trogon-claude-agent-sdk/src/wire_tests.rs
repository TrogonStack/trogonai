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

#[test]
fn malformed_permission_request_is_never_decoded_into_a_grantable_request() -> Result<(), ProtocolError> {
    for frame in [
        &br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Bash"}}"#[..],
        &br#"{"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{},"permission_suggestions":[{"destination":"session"}]}}"#[..],
    ] {
        assert!(matches!(
            parse_frame(frame)?,
            Frame::ControlRequest { id, request: IncomingControl::Malformed(_) } if id.as_str() == "permission"
        ));
    }
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
