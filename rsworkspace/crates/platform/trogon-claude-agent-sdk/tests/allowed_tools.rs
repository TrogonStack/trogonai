use trogon_claude_agent_sdk::{ClaudeAgentOptions, ToolRule};

#[test]
fn allowed_tool_rules_preserve_patterns_order_and_duplicates() {
    assert!(ClaudeAgentOptions::default().allowed_tools.is_empty());
    let values = ["Read", "Bash(git status:*)", "Read"];
    let options = ClaudeAgentOptions {
        allowed_tools: values.iter().map(|value| ToolRule::from(*value)).collect(),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(
        options.allowed_tools.iter().map(ToolRule::as_str).collect::<Vec<_>>(),
        values
    );
    assert_eq!(options.allowed_tools[1].to_string(), "Bash(git status:*)");
    assert_eq!(options.clone().allowed_tools, options.allowed_tools);
}
