use trogon_claude_agent_sdk::{McpToolName, McpToolNameError};

#[test]
fn mcp_tool_names_preserve_the_supported_alphabet_and_exact_maximum() -> Result<(), McpToolNameError> {
    for input in ["a".to_owned(), "Echo_01-tool.name".to_owned(), "a".repeat(128)] {
        let name = McpToolName::new(input.clone())?;
        assert_eq!(name.as_str(), input);
        assert_eq!(name.to_string(), input);
    }
    Ok(())
}

#[test]
fn mcp_tool_names_reject_invalid_lengths_and_characters() {
    for input in [
        "".to_owned(),
        "a".repeat(129),
        "two words".to_owned(),
        "café".to_owned(),
        "a:b".to_owned(),
        "a\0b".to_owned(),
    ] {
        assert!(matches!(McpToolName::new(input), Err(McpToolNameError)));
    }
}
