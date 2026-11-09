use trogon_claude_agent_sdk::{
    Content, ContentBlock, JsonObject, MessageId, MessageOrigin, SessionId, ToolUseId, UserMessage,
};

#[test]
fn user_messages_keep_text_blocks_injected_origin_and_tool_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let fields: JsonObject = serde_json::from_str(r#"{"future":{"kind":"extension"}}"#)?;
    let mut message = UserMessage {
        content: Content::Text("plain prompt".into()),
        parent_tool_use_id: Some(ToolUseId::new("parent-one")?),
        session_id: Some(SessionId::new("session-one")?),
        uuid: Some(MessageId::new("user-one")?),
        origin: Some(MessageOrigin::new("channel", fields.clone())?),
        tool_use_result: Some(fields.clone()),
        additional_fields: fields.clone(),
        message_fields: fields.clone(),
    };
    assert!(matches!(&message.content, Content::Text(text) if text == "plain prompt"));
    assert_eq!(message.origin.as_ref().map(MessageOrigin::kind), Some("channel"));
    message.content = Content::Blocks(vec![ContentBlock::Text {
        text: "block prompt".into(),
    }]);
    let copied = message.clone();
    assert!(matches!(&copied.content, Content::Blocks(blocks) if blocks.len() == 1));
    assert_eq!(copied.tool_use_result.as_ref(), Some(&fields));
    assert_eq!(copied.additional_fields, fields);
    assert_eq!(copied.uuid.as_ref().map(MessageId::as_str), Some("user-one"));
    Ok(())
}
