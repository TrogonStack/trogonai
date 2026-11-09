use trogon_claude_agent_sdk::{BuiltinToolName, BuiltinToolNameError, ToolName};

#[test]
fn builtin_names_preserve_open_ended_names_and_validate_converted_tool_names() -> Result<(), Box<dyn std::error::Error>>
{
    for input in ["Read", "FutureTool", "default_suffix", "outil-café"] {
        let value = BuiltinToolName::new(input)?;
        assert_eq!(value.as_str(), input);
        assert_eq!(BuiltinToolName::try_from(ToolName::new(input)?)?, value);
    }
    Ok(())
}

#[test]
fn builtin_names_reject_list_splitting_and_reserved_presets() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(BuiltinToolName::new(""), Err(BuiltinToolNameError::Empty)));
    for input in ["Read,Write", "Read Write", "Read\nWrite", "Read\0Write"] {
        assert!(matches!(
            BuiltinToolName::new(input),
            Err(BuiltinToolNameError::InvalidCharacter)
        ));
    }
    assert!(matches!(
        BuiltinToolName::new("default"),
        Err(BuiltinToolNameError::ReservedPreset)
    ));
    assert!(matches!(
        BuiltinToolName::try_from(ToolName::new("Read Write")?),
        Err(BuiltinToolNameError::InvalidCharacter)
    ));
    Ok(())
}
