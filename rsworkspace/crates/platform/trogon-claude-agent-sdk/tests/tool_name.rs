use trogon_claude_agent_sdk::{ToolName, ToolNameError};

#[test]
fn valid_opaque_values_are_preserved_without_format_restrictions() -> Result<(), ToolNameError> {
    for input in ["opaque-value", "unicode café value", "  "] {
        let value = ToolName::new(input)?;
        assert_eq!(value.as_str(), input);
        assert_eq!(value.clone(), value);
        assert_eq!(value.to_string(), input);
    }
    Ok(())
}

#[test]
fn empty_and_control_values_are_typed_construction_errors() {
    for input in ["", "value\nidentifier", "value\0identifier", "value\u{7f}"] {
        assert!(matches!(ToolName::new(input), Err(ToolNameError)));
    }
}
