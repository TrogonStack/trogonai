//! The TypeScript `coreTypes` namespace re-exports the declarations of the message, hook and control families; the
//! Rust equivalent is the `upstream` module tree, which holds them under the same names.

use serde_json::json;
use trogon_claude_agent_sdk::js_stringify;
use trogon_claude_agent_sdk::upstream::control::SDKControlRequest;
use trogon_claude_agent_sdk::upstream::hooks::HookEvent;
use trogon_claude_agent_sdk::upstream::messages::SDKMessage;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_message_decodes_as_the_core_types_union_and_round_trips() -> TestResult {
    let sample = json!({
        "type": "user",
        "message": {"role": "user", "content": "hello"},
        "parent_tool_use_id": null
    });
    let typed: SDKMessage = serde_json::from_value(sample.clone())?;
    assert!(format!("{typed:?}").starts_with("SDKUserMessage("), "decoded as {typed:?}");
    assert_eq!(js_stringify(&serde_json::to_value(&typed)?), js_stringify(&sample));
    Ok(())
}

#[test]
fn a_hook_event_name_decodes_as_the_core_types_union() -> TestResult {
    let typed: HookEvent = serde_json::from_value(json!("PreToolUse"))?;
    assert_eq!(serde_json::to_value(&typed)?, json!("PreToolUse"));
    Ok(())
}

#[test]
fn the_control_family_lives_next_to_the_core_types() -> TestResult {
    let sample = json!({"type": "control_request", "request_id": "r", "request": {"subtype": "interrupt"}});
    let typed: SDKControlRequest = serde_json::from_value(sample.clone())?;
    assert_eq!(serde_json::to_value(&typed)?, sample);
    Ok(())
}
