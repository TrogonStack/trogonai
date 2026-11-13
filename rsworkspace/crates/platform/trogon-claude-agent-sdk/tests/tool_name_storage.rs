use trogon_claude_agent_sdk::ToolName;

fn preserves_tool_name(input: &str) {
    let name = ToolName::from(input);
    assert_eq!(name.as_str(), input);
    assert_eq!(name.to_string(), input);
    assert_eq!(name.clone(), name);
}

#[test]
fn empty_tool_name_is_preserved() {
    preserves_tool_name("")
}

#[test]
fn newline_in_a_tool_name_is_preserved() {
    preserves_tool_name("tool\nname")
}

#[test]
fn nul_in_a_tool_name_is_preserved() {
    preserves_tool_name("tool\0name")
}

#[test]
fn delete_in_a_tool_name_is_preserved() {
    preserves_tool_name("tool\u{7f}name")
}
