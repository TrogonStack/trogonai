use trogon_claude_agent_sdk::DialogKind;

#[test]
fn valid_opaque_values_are_preserved_without_format_restrictions() {
    for input in ["opaque-value", "unicode café value", "  "] {
        let value = DialogKind::from(input);
        assert_eq!(value.as_str(), input);
        assert_eq!(value.clone(), value);
    }
}

#[test]
fn empty_and_control_values_are_stored_as_received() {
    for input in ["", "value\nidentifier", "value\0identifier", "value\u{7f}"] {
        assert_eq!(DialogKind::from(input).as_str(), input);
    }
}
