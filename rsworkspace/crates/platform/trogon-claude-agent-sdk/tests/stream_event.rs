use trogon_claude_agent_sdk::{JsonObject, SessionId, StreamEvent, ToolUseId};

#[test]
fn stream_events_keep_raw_event_values_and_parent_identity() -> Result<(), Box<dyn std::error::Error>> {
    let event: JsonObject =
        serde_json::from_str(r#"{"type":"content_block_delta","delta":{"text":"part"},"future":null}"#)?;
    let message = StreamEvent {
        uuid: "stream-one".into(),
        session_id: SessionId::new("session-one")?,
        event: event.clone(),
        parent_tool_use_id: Some(ToolUseId::new("parent-one")?),
    };
    let copied = message.clone();
    assert_eq!(copied.uuid, "stream-one");
    assert_eq!(copied.session_id.as_str(), "session-one");
    assert_eq!(copied.event, event);
    assert_eq!(
        copied.parent_tool_use_id.as_ref().map(ToolUseId::as_str),
        Some("parent-one")
    );
    Ok(())
}
