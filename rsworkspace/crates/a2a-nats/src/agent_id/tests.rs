use std::str::FromStr;

use super::*;

#[test]
fn a2a_agent_id_valid() {
    assert!(A2aAgentId::new("support-bot").is_ok());
    assert!(A2aAgentId::new("a").is_ok());
    assert_eq!(A2aAgentId::new("ok").unwrap().as_str(), "ok");
}

#[test]
fn a2a_agent_id_rejects_dots_wildcards_whitespace() {
    assert!(A2aAgentId::new("a.b").is_err());
    assert!(A2aAgentId::new("a*").is_err());
    assert!(A2aAgentId::new("a>").is_err());
    assert!(A2aAgentId::new("a b").is_err());
}

#[test]
fn a2a_agent_id_too_long() {
    let long_id = "a".repeat(129);
    assert!(A2aAgentId::new(&long_id).is_err());
    assert!(A2aAgentId::new("a".repeat(128).as_str()).is_ok());
}

#[test]
fn a2a_agent_id_empty_returns_err() {
    assert_eq!(A2aAgentId::new("").err().unwrap(), AgentIdError::Empty);
}

#[test]
fn a2a_agent_id_display_and_deref() {
    let id = A2aAgentId::new("my-agent").unwrap();
    assert_eq!(format!("{id}"), "my-agent");
    assert_eq!(id.len(), 8);
    assert!(id.starts_with("my"));
}

#[test]
fn a2a_agent_id_from_str_parses_valid_and_rejects_invalid() {
    let id: A2aAgentId = "from-str".parse().unwrap();
    assert_eq!(id.as_str(), "from-str");
    assert!(A2aAgentId::from_str("bad.dot").is_err());
}

#[test]
fn agent_id_error_display() {
    assert_eq!(AgentIdError::Empty.to_string(), "agent_id must not be empty");
    assert_eq!(
        AgentIdError::InvalidCharacter('.').to_string(),
        "agent_id contains invalid character: '.'"
    );
    assert_eq!(
        AgentIdError::TooLong(129).to_string(),
        "agent_id is too long: 129 characters (max 128)"
    );
}
