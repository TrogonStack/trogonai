use trogon_claude_agent_sdk::McpServerName;

#[test]
fn valid_opaque_values_are_preserved_without_format_restrictions() {
    for input in ["opaque-value", "unicode café value", "  "] {
        let value = McpServerName::from(input);
        assert_eq!(value.as_str(), input);
        assert_eq!(value.clone(), value);
        assert_eq!(value.to_string(), input);
    }
}

#[test]
fn owned_mcp_server_names_preserve_the_original_text() {
    let input = String::from("owned\0server");
    let value = McpServerName::from(input.clone());
    assert_eq!(value.as_str(), input);
    assert_eq!(value.to_string(), input);
}
