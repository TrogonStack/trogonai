use trogon_claude_agent_sdk::{
    Content, ContentBlock, JsonObject, MessageId, MessageOrigin, SessionId, ToolUseId, UserMessage,
};

#[test]
fn user_messages_keep_text_blocks_injected_origin_and_tool_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let fields: JsonObject = serde_json::from_str(r#"{"future":{"kind":"extension"}}"#)?;
    let mut message = UserMessage {
        content: Content::Text("plain prompt".into()),
        parent_tool_use_id: Some(ToolUseId::from("parent-one")),
        session_id: Some(SessionId::from("session-one")),
        uuid: Some(MessageId::from("user-one")),
        origin: Some(MessageOrigin::new("channel", fields.clone())?),
        tool_use_result: Some(serde_json::to_value(&fields)?),
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
    assert_eq!(copied.tool_use_result, Some(serde_json::to_value(&fields)?));
    assert_eq!(copied.additional_fields, fields);
    assert_eq!(copied.uuid.as_ref().map(MessageId::as_str), Some("user-one"));
    Ok(())
}

#[test]
fn user_tool_results_preserve_every_json_kind_and_omission() -> Result<(), Box<dyn std::error::Error>> {
    let mut message = UserMessage {
        content: Content::Text("prompt".into()),
        parent_tool_use_id: None,
        session_id: None,
        uuid: None,
        origin: None,
        tool_use_result: None,
        additional_fields: JsonObject::default(),
        message_fields: JsonObject::default(),
    };
    assert!(message.tool_use_result.is_none());
    for value in [
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!(42),
        serde_json::json!("result"),
        serde_json::json!([null, true, 42, "result", {"nested": []}]),
        serde_json::json!({"nested": [null, false]}),
    ] {
        message.tool_use_result = Some(serde_json::from_value(value.clone())?);
        assert_eq!(serde_json::to_value(message.clone().tool_use_result)?, value);
        assert!(message.tool_use_result.is_some());
    }
    Ok(())
}
