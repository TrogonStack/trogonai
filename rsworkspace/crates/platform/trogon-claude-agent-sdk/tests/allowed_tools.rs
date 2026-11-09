use trogon_claude_agent_sdk::{ClaudeAgentOptions, ToolRule, ToolRuleError};

#[test]
fn allowed_tool_rules_preserve_patterns_order_and_duplicates() -> Result<(), ToolRuleError> {
    assert!(ClaudeAgentOptions::default().allowed_tools.is_empty());
    let values = ["Read", "Bash(git status:*)", "Read"];
    let options = ClaudeAgentOptions {
        allowed_tools: values
            .iter()
            .map(|value| ToolRule::new(*value))
            .collect::<Result<_, _>>()?,
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(
        options.allowed_tools.iter().map(ToolRule::as_str).collect::<Vec<_>>(),
        values
    );
    assert_eq!(options.allowed_tools[1].to_string(), "Bash(git status:*)");
    assert_eq!(options.clone().allowed_tools, options.allowed_tools);
    Ok(())
}

#[test]
fn a_single_tool_rule_cannot_smuggle_another_rule_or_control_character() {
    for value in ["", "Read,Write", "Read\nWrite", "Read\0Write"] {
        assert!(matches!(ToolRule::new(value), Err(ToolRuleError)));
    }
}
