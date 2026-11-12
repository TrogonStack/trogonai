use super::*;

#[test]
fn malformed_known_content_does_not_become_unknown() {
    let frame = br#"{"type":"assistant","message":{"model":"test","content":[{"type":"tool_use","id":"id","name":"Bash","input":null}]}}"#;
    assert!(matches!(parse_frame(frame), Err(ProtocolError::Json(_))));
}

#[test]
fn future_informational_messages_are_preserved() -> Result<(), ProtocolError> {
    let frame = parse_frame(br#"{"type":"future_event","field":{"nested":1}}"#)?;
    assert!(matches!(frame, Frame::Message(Message::Unknown { kind, .. }) if kind == "future_event"));
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
