use trogon_claude_agent_sdk::ControlRequestId;

#[test]
fn valid_opaque_values_are_preserved_without_format_restrictions() {
    for input in ["opaque-value", "unicode café value", "  "] {
        let value = ControlRequestId::from(input);
        assert_eq!(value.as_str(), input);
        assert_eq!(value.clone(), value);
        assert_eq!(value.to_string(), input);
    }
}

#[test]
fn empty_and_control_values_are_stored_as_received() {
    for input in ["", "value\nidentifier", "value\0identifier", "value\u{7f}"] {
        let value = ControlRequestId::from(input);
        assert_eq!(value.as_str(), input);
        assert_eq!(value.to_string(), input);
    }
}

#[test]
fn empty_value_is_stored_as_received() {
    let input = "";
    let value = ControlRequestId::from(input);
    assert_eq!(value.as_str(), input);
}

#[test]
fn newline_value_is_stored_as_received() {
    let input = "value\nidentifier";
    let value = ControlRequestId::from(input);
    assert_eq!(value.as_str(), input);
}

#[test]
fn nul_value_is_stored_as_received() {
    let input = "value\0identifier";
    let value = ControlRequestId::from(input);
    assert_eq!(value.as_str(), input);
}

#[test]
fn delete_value_is_stored_as_received() {
    let input = "value\u{7f}";
    let value = ControlRequestId::from(input);
    assert_eq!(value.as_str(), input);
}
