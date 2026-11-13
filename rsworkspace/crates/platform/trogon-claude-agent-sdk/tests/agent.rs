use trogon_claude_agent_sdk::{AgentName, ClaudeAgentOptions};

#[test]
fn agent_selection_preserves_omission_and_named_values() {
    assert_eq!(ClaudeAgentOptions::default().agent, None);
    for value in ["reviewer", "agent with spaces", "agent-café"] {
        let name = AgentName::from(value);
        let options = ClaudeAgentOptions {
            agent: Some(name.clone()),
            ..ClaudeAgentOptions::default()
        };
        assert_eq!(name.as_str(), value);
        assert_eq!(options.clone().agent, Some(name));
    }
}

#[test]
fn agent_names_preserve_upstream_string_values() {
    for value in ["", "agent\nname", "agent\0name", "agent\u{7f}name"] {
        let name = AgentName::from(value);
        assert_eq!(name.as_str(), value);
    }
}
