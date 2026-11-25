#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;
use serde_json::json;

fn wire(input: &UserInput) -> Result<String, serde_json::Error> {
    serde_json::to_string(&input.to_wire_json()?)
}

fn object(value: Value) -> Result<JsonObject, Box<dyn std::error::Error>> {
    let Value::Object(object) = value else {
        return Err("not an object".into());
    };
    Ok(object)
}

/// `sdk.mjs` wraps a string prompt in a text block, Python writes it as a string; both put `session_id` ahead of
/// `message`. Any other input is a streamed message, which both write as given, with no session added.
#[test]
fn a_query_prompt_follows_each_sdk() -> Result<(), Box<dyn std::error::Error>> {
    let text = || UserInput::from(Prompt::from("hi"));
    assert_eq!(
        wire(&text().into_query_prompt(true))?,
        r#"{"type":"user","session_id":"","message":{"role":"user","content":[{"type":"text","text":"hi"}]},"parent_tool_use_id":null}"#
    );
    assert_eq!(
        wire(&text().into_query_prompt(false))?,
        r#"{"type":"user","session_id":"","message":{"role":"user","content":"hi"},"parent_tool_use_id":null}"#
    );
    let mut named = text().into_query_prompt(true);
    named.session_id = Some(SessionId::from("s"));
    named.uuid = Some(MessageId::from("u"));
    assert_eq!(
        wire(&named)?,
        r#"{"type":"user","session_id":"s","message":{"role":"user","content":[{"type":"text","text":"hi"}]},"parent_tool_use_id":null,"uuid":"u"}"#
    );
    for text_block in [true, false] {
        let blocks = UserInput::blocks(vec![InputBlock::text("b")]).into_query_prompt(text_block);
        assert_eq!(
            wire(&blocks)?,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"b"}]},"parent_tool_use_id":null}"#
        );
    }
    let raw = UserInput::from_json(object(json!({"type": "user", "x": 1}))?).into_query_prompt(true);
    assert_eq!(wire(&raw)?, r#"{"type":"user","x":1}"#);
    Ok(())
}

/// Python `ClaudeSDKClient.query`: only a message with no `session_id` key takes the session, appended last.
#[test]
fn fill_session_names_only_an_unnamed_message() -> Result<(), Box<dyn std::error::Error>> {
    let session = SessionId::from("q");
    let mut unnamed = UserInput::from(Prompt::from("hi"));
    unnamed.additional_fields = object(json!({"extra": 1}))?;
    unnamed.fill_session(&session);
    assert_eq!(
        wire(&unnamed)?,
        r#"{"type":"user","message":{"role":"user","content":"hi"},"parent_tool_use_id":null,"extra":1,"session_id":"q"}"#
    );
    let mut typed = UserInput::from(Prompt::from("hi"));
    typed.session_id = Some(SessionId::from("keep"));
    typed.fill_session(&session);
    assert_eq!(typed.to_wire_json()?["session_id"], "keep");
    let mut extension = UserInput::from(Prompt::from("hi"));
    extension.additional_fields = object(json!({"session_id": "keep"}))?;
    extension.fill_session(&session);
    assert_eq!(extension.to_wire_json()?["session_id"], "keep");
    let mut raw = UserInput::from_json(object(json!({"session_id": null, "type": "user"}))?);
    raw.fill_session(&session);
    assert_eq!(wire(&raw)?, r#"{"session_id":null,"type":"user"}"#);
    let mut raw = UserInput::from_json(object(json!({"type": "user"}))?);
    raw.fill_session(&session);
    assert_eq!(wire(&raw)?, r#"{"type":"user","session_id":"q"}"#);
    Ok(())
}
