use trogon_claude_agent_sdk::{AssistantMessage, ContentBlock, JsonObject, MessageId, SessionId, ToolUseId};

#[test]
fn assistant_messages_keep_model_content_ids_and_both_extension_scopes() -> Result<(), Box<dyn std::error::Error>> {
    let outer: JsonObject = serde_json::from_str(r#"{"outer_future":true}"#)?;
    let nested: JsonObject = serde_json::from_str(r#"{"nested_future":null}"#)?;
    let message = AssistantMessage {
        content: vec![ContentBlock::Text { text: "answer".into() }],
        model: "provider/model release".into(),
        parent_tool_use_id: Some(ToolUseId::from("parent-one")),
        session_id: Some(SessionId::from("session-one")),
        error: Some("future_error".into()),
        usage: Some(nested.clone()),
        uuid: Some(MessageId::from("frame-one")),
        message_id: Some(MessageId::from("message-one")),
        stop_reason: None,
        additional_fields: outer.clone(),
        message_fields: nested.clone(),
        raw: JsonObject::new(),
    };
    let copied = message.clone();
    assert_eq!(copied.model, "provider/model release");
    assert!(matches!(&copied.content[0], ContentBlock::Text { text } if text == "answer"));
    assert_eq!(
        copied.parent_tool_use_id.as_ref().map(ToolUseId::as_str),
        Some("parent-one")
    );
    assert_eq!(copied.session_id.as_ref().map(SessionId::as_str), Some("session-one"));
    assert_eq!(copied.uuid.as_ref().map(MessageId::as_str), Some("frame-one"));
    assert_eq!(copied.message_id.as_ref().map(MessageId::as_str), Some("message-one"));
    assert_eq!(copied.error.as_deref(), Some("future_error"));
    assert_eq!(copied.additional_fields, outer);
    assert_eq!(copied.message_fields, nested);
    Ok(())
}
