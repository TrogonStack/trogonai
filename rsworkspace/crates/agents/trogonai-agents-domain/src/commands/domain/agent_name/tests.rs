use super::*;

#[test]
fn accepts_nonblank_trimmed_name() {
    let name = AgentName::parse("pr-reviewer").unwrap();
    assert_eq!(name.as_str(), "pr-reviewer");
}

#[test]
fn rejects_empty_and_surrounding_whitespace() {
    assert!(AgentName::parse("").is_err());
    assert!(AgentName::parse(" pr-reviewer").is_err());
    assert!(AgentName::parse("pr-reviewer ").is_err());
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let value = AgentName::parse("pr-reviewer").unwrap();

    assert_eq!(value.as_ref(), "pr-reviewer");
    assert_eq!("pr-reviewer".parse::<AgentName>().unwrap().as_str(), "pr-reviewer");
    assert_eq!(value.to_string(), "pr-reviewer");
}
