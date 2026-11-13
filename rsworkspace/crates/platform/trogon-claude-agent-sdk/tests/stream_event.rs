use trogon_claude_agent_sdk::{JsonObject, SessionId, StreamEvent, ToolUseId};

#[test]
fn stream_events_keep_raw_event_values_and_parent_identity() -> Result<(), Box<dyn std::error::Error>> {
    let event: JsonObject =
        serde_json::from_str(r#"{"type":"content_block_delta","delta":{"text":"part"},"future":null}"#)?;
    let fields: JsonObject = serde_json::from_value(serde_json::json!({
        "ttft_ms": 12.5,
        "user_message_uuid": "user-one",
        "user_message_uuids": ["user-one", "user-two"],
        "resume_reason": "future-reason",
        "future": {"nested": [null, true]}
    }))?;
    let message = StreamEvent {
        uuid: "stream-one".into(),
        session_id: SessionId::from("session-one"),
        event: event.clone(),
        additional_fields: fields.clone(),
        parent_tool_use_id: Some(ToolUseId::from("parent-one")),
    };
    let copied = message.clone();
    assert_eq!(copied.uuid, "stream-one");
    assert_eq!(copied.session_id.as_str(), "session-one");
    assert_eq!(copied.event, event);
    assert_eq!(copied.additional_fields, fields);
    assert!(!copied.event.contains_key("ttft_ms"));
    assert_eq!(copied.event["future"], serde_json::Value::Null);
    assert_eq!(
        copied.parent_tool_use_id.as_ref().map(ToolUseId::as_str),
        Some("parent-one")
    );
    Ok(())
}
