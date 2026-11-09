use trogon_claude_agent_sdk::{BuiltinToolName, ToolName};

#[test]
fn builtin_names_preserve_open_ended_names_and_converted_tool_names() -> Result<(), Box<dyn std::error::Error>> {
    for input in ["Read", "FutureTool", "default_suffix", "outil-café"] {
        let value = BuiltinToolName::new(input);
        assert_eq!(value.as_str(), input);
        assert_eq!(BuiltinToolName::from(ToolName::new(input)?), value);
    }
    Ok(())
}

#[test]
fn owned_builtin_tool_names_preserve_the_original_text() {
    let input = String::from("Read,\0Write");
    let value = BuiltinToolName::new(input.clone());
    assert_eq!(value.as_str(), input);
}
