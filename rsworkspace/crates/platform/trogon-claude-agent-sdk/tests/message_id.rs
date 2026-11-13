use trogon_claude_agent_sdk::MessageId;

#[test]
fn valid_opaque_values_are_preserved_without_format_restrictions() {
    for input in ["opaque-value", "unicode café value", "  "] {
        let value = MessageId::from(input);
        assert_eq!(value.as_str(), input);
        assert_eq!(value.clone(), value);
        assert_eq!(value.to_string(), input);
    }
}
