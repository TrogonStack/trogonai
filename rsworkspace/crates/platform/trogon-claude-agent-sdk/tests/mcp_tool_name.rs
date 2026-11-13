use trogon_claude_agent_sdk::McpToolName;

#[test]
fn mcp_tool_names_preserve_recommended_spellings_and_length() {
    for input in ["a".to_owned(), "Echo_01-tool.name".to_owned(), "a".repeat(128)] {
        let name = McpToolName::from(input.clone());
        assert_eq!(name.as_str(), input);
        assert_eq!(name.to_string(), input);
    }
}

#[test]
fn mcp_tool_names_preserve_values_outside_naming_recommendations() {
    for input in [
        "".to_owned(),
        "a".repeat(129),
        "two words".to_owned(),
        "café".to_owned(),
        "a:b".to_owned(),
        "a\0b".to_owned(),
    ] {
        let name = McpToolName::from(input.clone());
        assert_eq!(name.as_str(), input);
        assert_eq!(name.to_string(), input);
    }
}
