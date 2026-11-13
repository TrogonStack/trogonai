use trogon_claude_agent_sdk::ToolUseId;

fn preserves_tool_use_id(input: &str) {
    let id = ToolUseId::from(input);
    assert_eq!(id.as_str(), input);
    assert_eq!(id.to_string(), input);
    assert_eq!(id.clone(), id);
}

#[test]
fn empty_tool_use_id_is_preserved() {
    preserves_tool_use_id("")
}

#[test]
fn newline_in_a_tool_use_id_is_preserved() {
    preserves_tool_use_id("id\nvalue")
}

#[test]
fn nul_in_a_tool_use_id_is_preserved() {
    preserves_tool_use_id("id\0value")
}

#[test]
fn delete_in_a_tool_use_id_is_preserved() {
    preserves_tool_use_id("id\u{7f}value")
}
