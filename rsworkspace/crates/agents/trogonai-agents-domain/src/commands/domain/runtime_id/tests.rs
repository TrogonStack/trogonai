use super::*;

#[test]
fn validates_runtime_id() {
    assert_eq!(RuntimeId::parse("claude-code").unwrap().as_str(), "claude-code");
    assert!(RuntimeId::parse("").is_err());
    assert!(RuntimeId::parse(" claude-code").is_err());
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let value = RuntimeId::parse("claude-code").unwrap();

    assert_eq!(value.as_ref(), "claude-code");
    assert_eq!("claude-code".parse::<RuntimeId>().unwrap().as_str(), "claude-code");
    assert_eq!(value.to_string(), "claude-code");
}
