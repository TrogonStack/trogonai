use trogon_claude_agent_sdk::{BuiltinToolName, ToolName};

fn preserves_builtin_tool_name(input: &str) {
    let name = BuiltinToolName::from(input);
    assert_eq!(name.as_str(), input);
    assert_eq!(name.clone(), name);
}

fn preserves_tool_name_conversion(input: &str) {
    let source = ToolName::from(input);
    let name = BuiltinToolName::from(source);
    assert_eq!(name.as_str(), input);
}

#[test]
fn empty_builtin_tool_name_is_preserved() {
    preserves_builtin_tool_name("")
}

#[test]
fn comma_in_a_builtin_tool_name_is_preserved() {
    preserves_builtin_tool_name("Read,Write")
}

#[test]
fn whitespace_in_a_builtin_tool_name_is_preserved() {
    preserves_builtin_tool_name("with space")
}

#[test]
fn newline_in_a_builtin_tool_name_is_preserved() {
    preserves_builtin_tool_name("with\ncontrol")
}

#[test]
fn nul_in_a_builtin_tool_name_is_preserved() {
    preserves_builtin_tool_name("with\0control")
}

#[test]
fn default_in_a_builtin_tool_name_is_preserved() {
    preserves_builtin_tool_name("default")
}

#[test]
fn converted_tool_name_preserves_comma() {
    preserves_tool_name_conversion("Read,Write")
}

#[test]
fn converted_tool_name_preserves_whitespace() {
    preserves_tool_name_conversion("with space")
}

#[test]
fn converted_tool_name_preserves_default() {
    preserves_tool_name_conversion("default")
}
