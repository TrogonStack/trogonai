#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    InputBlock, JsonObject, MessageId, MessageOrigin, Prompt, SessionId, ToolUseId, UserInput, UserInputPriority,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn object(value: Value) -> Result<JsonObject, Box<dyn std::error::Error>> {
    let Value::Object(object) = value else {
        return Err("not an object".into());
    };
    Ok(object)
}

/// A text input is an `SDKUserMessage` with string content: `type`, `message`, `parent_tool_use_id`, and no session
/// until one is named.
#[test]
fn text_input_serializes_as_a_user_message_without_a_session() -> TestResult {
    let wire = UserInput::from(Prompt::from("hello".to_owned())).to_wire_json()?;
    assert_eq!(
        serde_json::to_string(&wire)?,
        r#"{"type":"user","message":{"role":"user","content":"hello"},"parent_tool_use_id":null}"#
    );
    Ok(())
}

/// Each set field follows the order `SDKUserMessage` declares, and extension keys come last.
#[test]
fn structured_input_carries_blocks_and_every_field_in_declaration_order() -> TestResult {
    let image = object(json!({"type": "image", "source": {"type": "url", "url": "u"}}))?;
    let mut input = UserInput::blocks(vec![InputBlock::text("inspect"), InputBlock::from(image)]);
    input.additional_fields = object(json!({"zeta": 1, "alpha": 2}))?;
    input.task_description = Some("task".into());
    input.subagent_type = Some("kind".into());
    input.agent_id = Some("agent".into());
    input.session_id = Some(SessionId::from("session-1"));
    input.inline_pastes = Some(vec!["paste".into()]);
    input.pasted_content = Some(vec![json!("pasted")]);
    input.uuid = Some(MessageId::from("message-1"));
    input.client_composed = Some(true);
    input.timestamp = Some("2026-01-01T00:00:00.000Z".into());
    input.should_query = Some(true);
    input.origin = Some(MessageOrigin::new("task", JsonObject::new()));
    input.priority = Some(UserInputPriority::Later);
    input.tool_use_result = Some(json!(null));
    input.is_synthetic = Some(false);
    input.parent_tool_use_id = Some(ToolUseId::from("tool-1".to_owned()));
    assert_eq!(
        serde_json::to_string(&input.to_wire_json()?)?,
        concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"inspect"},"#,
            r#"{"type":"image","source":{"type":"url","url":"u"}}]},"parent_tool_use_id":"tool-1","#,
            r#""isSynthetic":false,"tool_use_result":null,"priority":"later","origin":{"kind":"task"},"#,
            r#""shouldQuery":true,"timestamp":"2026-01-01T00:00:00.000Z","client_composed":true,"#,
            r#""uuid":"message-1","pasted_content":["pasted"],"inline_pastes":["paste"],"#,
            r#""session_id":"session-1","agent_id":"agent","subagent_type":"kind","task_description":"task","#,
            r#""zeta":1,"alpha":2}"#
        )
    );
    assert_eq!(InputBlock::text("x").as_json().get("type"), Some(&json!("text")));
    Ok(())
}

/// A message object is written as given, unknown keys and key order included. A typed field replaces its key in
/// place or is added at the end, and an extension key never replaces a key the message already has.
#[test]
fn a_message_object_is_written_as_given_with_typed_fields_overlaid() -> TestResult {
    let given = r#"{"future":1,"uuid":"old","message":{"content":"raw","role":"user"},"type":"user"}"#;
    let input = UserInput::from_json(serde_json::from_str(given)?);
    assert_eq!(serde_json::to_string(&input.to_wire_json()?)?, given);
    let mut input = UserInput::from_json(serde_json::from_str(given)?);
    input.uuid = Some(MessageId::from("new"));
    input.priority = Some(UserInputPriority::Now);
    input.additional_fields = object(json!({"future": 2, "extra": true}))?;
    assert_eq!(
        serde_json::to_string(&input.to_wire_json()?)?,
        r#"{"future":1,"uuid":"new","message":{"content":"raw","role":"user"},"type":"user","priority":"now","extra":true}"#
    );
    Ok(())
}

/// Both SDKs write `priority` unvalidated, so an unknown spelling round-trips unchanged.
#[test]
fn user_input_priorities_round_trip_upstream_literals() -> Result<(), serde_json::Error> {
    for (priority, literal) in [
        (UserInputPriority::Now, "now"),
        (UserInputPriority::Next, "next"),
        (UserInputPriority::Later, "later"),
        (UserInputPriority::Unknown("soon".into()), "soon"),
        (UserInputPriority::Unknown("Now".into()), "Now"),
    ] {
        assert_eq!(priority.as_str(), literal);
        assert_eq!(UserInputPriority::from(literal.to_owned()), priority);
        assert_eq!(serde_json::to_value(&priority)?, json!(literal));
        assert_eq!(serde_json::from_value::<UserInputPriority>(json!(literal))?, priority);
    }
    Ok(())
}
