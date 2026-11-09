use trogon_claude_agent_sdk::{Prompt, PromptError};

#[test]
fn prompts_preserve_multiline_whitespace_and_unicode_text() -> Result<(), PromptError> {
    for input in ["hello", "line one\nline two", "\t  ", "bonjour café"] {
        let prompt = Prompt::from(input)?;
        assert_eq!(prompt.as_str(), input);
        assert_eq!(prompt.to_string(), input);
        assert_eq!(prompt.clone(), prompt);
    }
    Ok(())
}

#[test]
fn empty_and_nul_prompts_fail_with_a_typed_error() {
    for input in ["", "before\0after"] {
        assert!(matches!(Prompt::from(input), Err(PromptError)));
    }
}

fn assert_upstream_prompt_round_trip(input: &str) {
    let prompt = Prompt::from(input).expect("upstream prompt strings must remain valid");
    assert_eq!(prompt.as_str(), input);
    let encoded = serde_json::to_string(prompt.as_str()).unwrap();
    let decoded: String = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, input);
    assert!(!encoded.contains('\0'));
    assert!(!encoded.contains('\n'));
}

#[test]
fn empty_prompt_strings_remain_lossless_in_json() {
    assert_upstream_prompt_round_trip("");
}

#[test]
fn nul_prompt_strings_remain_lossless_in_json() {
    assert_upstream_prompt_round_trip("before\0after");
}
