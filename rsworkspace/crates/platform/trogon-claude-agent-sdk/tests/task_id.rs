use trogon_claude_agent_sdk::{TaskId, TaskIdError};

#[test]
fn valid_opaque_values_are_preserved_without_format_restrictions() -> Result<(), TaskIdError> {
    for input in ["opaque-value", "unicode café value", "  "] {
        let value = TaskId::from(input)?;
        assert_eq!(value.as_str(), input);
        assert_eq!(value.clone(), value);
    }
    Ok(())
}

#[test]
fn empty_and_control_values_are_typed_construction_errors() {
    for input in ["", "value\nidentifier", "value\0identifier", "value\u{7f}"] {
        assert!(matches!(TaskId::from(input), Err(TaskIdError)));
    }
}

#[test]
fn empty_value_is_stored_as_received() {
    let input = "";
    let value = TaskId::from(input).unwrap_or_else(|_| panic!("empty value was rejected"));
    assert_eq!(value.as_str(), input);
}

#[test]
fn newline_value_is_stored_as_received() {
    let input = "value\nidentifier";
    let value = TaskId::from(input).unwrap_or_else(|_| panic!("newline value was rejected"));
    assert_eq!(value.as_str(), input);
}

#[test]
fn nul_value_is_stored_as_received() {
    let input = "value\0identifier";
    let value = TaskId::from(input).unwrap_or_else(|_| panic!("NUL value was rejected"));
    assert_eq!(value.as_str(), input);
}

#[test]
fn delete_value_is_stored_as_received() {
    let input = "value\u{7f}";
    let value = TaskId::from(input).unwrap_or_else(|_| panic!("DEL value was rejected"));
    assert_eq!(value.as_str(), input);
}
