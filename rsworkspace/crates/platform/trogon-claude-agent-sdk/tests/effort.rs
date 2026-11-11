use trogon_claude_agent_sdk::{ClaudeAgentOptions, EffortLevel};

#[test]
fn effort_preserves_omission_and_the_exact_upstream_literals() {
    assert_eq!(ClaudeAgentOptions::default().effort, None);
    for (level, literal) in [
        (EffortLevel::Low, "low"),
        (EffortLevel::Medium, "medium"),
        (EffortLevel::High, "high"),
        (EffortLevel::XHigh, "xhigh"),
        (EffortLevel::Max, "max"),
    ] {
        assert_eq!(level.as_str(), literal);
        let options = ClaudeAgentOptions {
            effort: Some(level),
            ..ClaudeAgentOptions::default()
        };
        assert_eq!(options.clone().effort, Some(level));
    }
}
