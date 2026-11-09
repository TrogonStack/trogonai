use trogon_claude_agent_sdk::{BuiltinToolName, ToolName, ToolNameError};

fn preserves_builtin_tool_name(input: &str) {
    let name = BuiltinToolName::new(input);
    assert_eq!(name.as_str(), input);
    assert_eq!(name.clone(), name);
}

fn preserves_tool_name_conversion(input: &str) -> Result<(), ToolNameError> {
    let source = ToolName::new(input)?;
    let name = BuiltinToolName::from(source);
    assert_eq!(name.as_str(), input);
    Ok(())
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
fn converted_tool_name_preserves_comma() -> Result<(), ToolNameError> {
    preserves_tool_name_conversion("Read,Write")
}

#[test]
fn converted_tool_name_preserves_whitespace() -> Result<(), ToolNameError> {
    preserves_tool_name_conversion("with space")
}

#[test]
fn converted_tool_name_preserves_default() -> Result<(), ToolNameError> {
    preserves_tool_name_conversion("default")
}
