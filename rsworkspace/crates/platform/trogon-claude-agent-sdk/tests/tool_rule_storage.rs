use trogon_claude_agent_sdk::{ClaudeAgentOptions, ToolRule};

fn preserves_rule_storage(input: &str) {
    let rule = ToolRule::from(input);
    let options = ClaudeAgentOptions {
        allowed_tools: vec![rule.clone()],
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(rule.as_str(), input);
    assert_eq!(rule.to_string(), input);
    assert_eq!(options.clone().allowed_tools[0].as_str(), input);
}

#[test]
fn empty_tool_rule_is_preserved_during_configuration() {
    preserves_rule_storage("")
}

#[test]
fn comma_in_a_tool_rule_is_preserved_during_configuration() {
    preserves_rule_storage("Bash(echo a,b)")
}

#[test]
fn newline_in_a_tool_rule_is_preserved_during_configuration() {
    preserves_rule_storage("Read\nWrite")
}

#[test]
fn nul_in_a_tool_rule_is_preserved_during_configuration() {
    preserves_rule_storage("Read\0Write")
}
