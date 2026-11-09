use trogon_claude_agent_sdk::{Prompt, PromptError};

#[test]
fn prompts_preserve_multiline_whitespace_and_unicode_text() -> Result<(), PromptError> {
    for input in ["hello", "line one\nline two", "\t  ", "bonjour café"] {
        let prompt = Prompt::new(input)?;
        assert_eq!(prompt.as_str(), input);
        assert_eq!(prompt.to_string(), input);
        assert_eq!(prompt.clone(), prompt);
    }
    Ok(())
}

#[test]
fn empty_and_nul_prompts_fail_with_a_typed_error() {
    for input in ["", "before\0after"] {
        assert!(matches!(Prompt::new(input), Err(PromptError)));
    }
}
