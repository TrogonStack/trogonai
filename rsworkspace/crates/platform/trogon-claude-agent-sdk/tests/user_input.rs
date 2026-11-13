#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use serde_json::json;
use trogon_claude_agent_sdk::{
    InputBlock, JsonObject, MessageId, MessageOrigin, Prompt, SessionId, ToolUseId, UserInput,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn text_input_serializes_as_a_user_message_with_empty_identity() -> TestResult {
    let wire = UserInput::from(Prompt::from("hello".to_owned())).to_wire_json()?;
    assert_eq!(
        wire,
        json!({"type": "user", "session_id": "", "parent_tool_use_id": null,
               "message": {"role": "user", "content": "hello"}})
    );
    Ok(())
}

#[test]
fn structured_input_carries_blocks_and_every_identity_field() -> TestResult {
    let image: JsonObject = serde_json::from_value(json!({"type": "image", "source": {"type": "url", "url": "u"}}))?;
    let mut input = UserInput::blocks(vec![InputBlock::text("inspect"), InputBlock::from(image.clone())]);
    input.uuid = Some(MessageId::from("message-1"));
    input.session_id = Some(SessionId::from("session-1"));
    input.parent_tool_use_id = Some(ToolUseId::from("tool-1".to_owned()));
    input.origin = Some(MessageOrigin::new("task", JsonObject::new()));
    let wire = input.to_wire_json()?;
    assert_eq!(wire["session_id"], "session-1");
    assert_eq!(wire["parent_tool_use_id"], "tool-1");
    assert_eq!(wire["uuid"], "message-1");
    assert_eq!(wire["origin"]["kind"], "task");
    assert_eq!(
        wire["message"]["content"][0],
        json!({"type": "text", "text": "inspect"})
    );
    assert_eq!(wire["message"]["content"][1], serde_json::Value::Object(image));
    assert_eq!(InputBlock::text("x").as_json().get("type"), Some(&json!("text")));
    Ok(())
}
