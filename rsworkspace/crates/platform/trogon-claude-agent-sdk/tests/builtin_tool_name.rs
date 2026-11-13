use trogon_claude_agent_sdk::{BuiltinToolName, ToolName};

#[test]
fn builtin_names_preserve_open_ended_names_and_converted_tool_names() {
    for input in ["Read", "FutureTool", "default_suffix", "outil-café"] {
        let value = BuiltinToolName::from(input);
        assert_eq!(value.as_str(), input);
        assert_eq!(BuiltinToolName::from(ToolName::from(input)), value);
    }
}

#[test]
fn owned_builtin_tool_names_preserve_the_original_text() {
    let input = String::from("Read,\0Write");
    let value = BuiltinToolName::from(input.clone());
    assert_eq!(value.as_str(), input);
}
