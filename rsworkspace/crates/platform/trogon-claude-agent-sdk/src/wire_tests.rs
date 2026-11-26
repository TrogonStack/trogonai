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

#[test]
fn remote_control_work_secret_keeps_the_raw_session_id() -> Result<(), ProtocolError> {
    let parse = |request: &str| {
        parse_frame(format!(r#"{{"type":"control_request","request_id":"r","request":{request}}}"#).as_bytes())
    };
    assert!(matches!(
        parse(r#"{"subtype":"remote_control_work_secret","session_id":5}"#)?,
        Frame::ControlRequest {
            request: IncomingControl::RemoteControlWork { session_id: Some(_) },
            ..
        }
    ));
    assert!(matches!(
        parse(r#"{"subtype":"remote_control_work_secret"}"#)?,
        Frame::ControlRequest {
            request: IncomingControl::RemoteControlWork { session_id: None },
            ..
        }
    ));
    Ok(())
}

#[test]
fn ui_requests_keep_their_payload_and_other_ui_subtypes_are_unsupported() -> Result<(), ProtocolError> {
    let parse = |request: &str| {
        parse_frame(format!(r#"{{"type":"control_request","request_id":"r","request":{request}}}"#).as_bytes())
    };
    let Frame::ControlRequest {
        request: IncomingControl::Ui { subtype, payload },
        ..
    } = parse(r#"{"subtype":"ui_copy","text":"hi"}"#)?
    else {
        return Err(ProtocolError::InvalidMessage("expected a ui request".to_owned()));
    };
    assert_eq!(subtype, "ui_copy");
    assert_eq!(payload.len(), 1);
    assert_eq!(payload.get("text"), Some(&Value::from("hi")));
    assert!(matches!(
        parse(r#"{"subtype":"ui_other"}"#)?,
        Frame::ControlRequest {
            request: IncomingControl::Unsupported { .. },
            ..
        }
    ));
    Ok(())
}

/// Containers deeper than the native stack of a test thread holds a recursive walk over.
const DEEP: usize = 100_000;

/// Whether `value` holds the string `leaf` [`DEEP`] containers down. No string in it may be a detached marker.
fn holds_deep_leaf(value: &Value) -> bool {
    let mut found = false;
    let mut pending = vec![(value, 0)];
    while let Some((value, depth)) = pending.pop() {
        match value {
            Value::String(text) => {
                assert!(
                    !text.starts_with("\u{0}detached:"),
                    "a detached marker stayed in the frame"
                );
                found |= text == "leaf" && depth >= DEEP;
            }
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(members) => pending.extend(members.values().map(|member| (member, depth + 1))),
            _ => {}
        }
    }
    found
}

#[test]
fn every_frame_decodes_routes_and_drops_containers_nested_past_the_native_stack()
-> Result<(), Box<dyn std::error::Error>> {
    let deep = format!("{}\"leaf\"{}", "[".repeat(DEEP), "]".repeat(DEEP));
    let request = |body: &str| format!(r#"{{"type":"control_request","request_id":"r","request":{body}}}"#);
    let cases = [
        (
            r#"{"type":"system","subtype":"deep","payload":DEEP}"#.to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"system","subtype":"deep","payload":DEEP}"#.to_owned(),
            MessageDecoding::Python,
            true,
        ),
        (
            r#"{"type":"future","payload":DEEP}"#.to_owned(),
            MessageDecoding::Python,
            false,
        ),
        (
            r#"{"type":"keep_alive","payload":DEEP}"#.to_owned(),
            MessageDecoding::TypeScript,
            false,
        ),
        (
            request(
                r#"{"subtype":"can_use_tool","tool_name":"Bash","input":{"a":DEEP},
                    "permission_suggestions":[{"type":"future","a":DEEP}],"a":DEEP}"#,
            ),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"can_use_tool","input":DEEP}"#),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"hook_callback","callback_id":"c","input":{"a":DEEP}}"#),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"mcp_message","server_name":DEEP,"message":DEEP}"#),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"elicitation","mcp_server_name":"s","message":"m","requested_schema":{"a":DEEP}}"#),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"request_user_dialog","dialog_kind":"k","payload":{"a":DEEP}}"#),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"future","a":DEEP}"#),
            MessageDecoding::TypeScript,
            false,
        ),
        (
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"r","response":{"a":DEEP}}}"#
                .to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"r","error":"e"},"a":DEEP}"#
                .to_owned(),
            MessageDecoding::TypeScript,
            false,
        ),
        (
            r#"{"type":"control_cancel_request","request_id":"r","a":DEEP}"#.to_owned(),
            MessageDecoding::TypeScript,
            false,
        ),
        (
            r#"{"type":"transcript_mirror","filePath":"f","entries":[{"a":DEEP}]}"#.to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"assistant","message":{"model":"m","content":[]},"a":DEEP}"#.to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"user","message":{"content":"hi"},"a":DEEP}"#.to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,
                "num_turns":1,"session_id":"s","a":DEEP}"#
                .to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"stream_event","uuid":"u","session_id":"s","event":{"a":DEEP}}"#.to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","a":DEEP},"uuid":"u","session_id":"s"}"#
                .to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"conversation_reset","new_conversation_id":"c","uuid":"u","session_id":"s","a":DEEP}"#
                .to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            r#"{"type":"future","payload":DEEP}"#.to_owned(),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"oauth_token_refresh","kind":DEEP}"#),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"host_auth_token_refresh","status":DEEP}"#),
            MessageDecoding::TypeScript,
            true,
        ),
        (
            request(r#"{"subtype":"remote_control_work_secret","session_id":DEEP}"#),
            MessageDecoding::TypeScript,
            true,
        ),
    ];
    // A failed assertion would unwind through the recursive drop of the deep values, so each case is checked
    // only after its values are dropped without recursion.
    let mut mismatches = Vec::new();
    for (text, mode, routed) in cases {
        let mut object = crate::deep_json::parse_object(text.replace("DEEP", &deep).as_bytes())?;
        let mut frame = parse_value(&object, mode)?;
        let mut found = false;
        frame.each_value(&mut |value| found |= holds_deep_leaf(value));
        frame.drop_values();
        crate::deep_json::clear_object(&mut object);
        if found != routed {
            mismatches.push(format!("{text}: routed {found}, expected {routed}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}
