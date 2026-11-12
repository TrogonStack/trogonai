use trogon_claude_agent_sdk::{ClaudeAgentOptions, ToolRule};

#[test]
fn disallowed_rules_preserve_order_and_remain_independent_of_allowed_rules() {
    assert!(ClaudeAgentOptions::default().disallowed_tools.is_empty());
    let options = ClaudeAgentOptions {
        allowed_tools: vec![ToolRule::from("Read")],
        disallowed_tools: vec![
            ToolRule::from("Bash(*)"),
            ToolRule::from("Read"),
            ToolRule::from("Bash(*)"),
        ],
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(
        options
            .disallowed_tools
            .iter()
            .map(ToolRule::as_str)
            .collect::<Vec<_>>(),
        ["Bash(*)", "Read", "Bash(*)"]
    );
    let mut cloned = options.clone();
    cloned.disallowed_tools.clear();
    assert!(cloned.disallowed_tools.is_empty());
    assert_eq!(options.disallowed_tools.len(), 3);
    assert_eq!(cloned.allowed_tools, options.allowed_tools);
}
