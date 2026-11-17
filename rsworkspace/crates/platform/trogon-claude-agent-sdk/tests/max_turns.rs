use std::num::NonZeroUsize;
use trogon_claude_agent_sdk::ClaudeAgentOptions;

#[test]
fn turn_limit_preserves_omission_and_explicit_positive_counts() {
    assert_eq!(ClaudeAgentOptions::default().max_turns, None);
    assert!(NonZeroUsize::new(0).is_none());
    for count in [1, 5, usize::MAX] {
        let options = ClaudeAgentOptions {
            max_turns: NonZeroUsize::new(count),
            ..ClaudeAgentOptions::default()
        };
        assert_eq!(options.max_turns.map(NonZeroUsize::get), Some(count));
        assert_eq!(options.clone().max_turns, options.max_turns);
    }
}
