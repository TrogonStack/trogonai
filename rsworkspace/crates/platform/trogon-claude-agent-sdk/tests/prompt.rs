use trogon_claude_agent_sdk::Prompt;

#[test]
fn prompts_preserve_multiline_whitespace_and_unicode_text() {
    for input in ["hello", "line one\nline two", "\t  ", "bonjour café"] {
        let prompt = Prompt::from(input);
        assert_eq!(prompt.as_str(), input);
        assert_eq!(prompt.to_string(), input);
        assert_eq!(prompt.clone(), prompt);
    }
}

fn assert_upstream_prompt_round_trip(input: &str) -> serde_json::Result<()> {
    let prompt = Prompt::from(input);
    assert_eq!(prompt.as_str(), input);
    let encoded = serde_json::to_string(prompt.as_str())?;
    let decoded: String = serde_json::from_str(&encoded)?;
    assert_eq!(decoded, input);
    assert!(!encoded.contains('\0'));
    assert!(!encoded.contains('\n'));
    Ok(())
}

#[test]
fn empty_prompt_strings_remain_lossless_in_json() -> serde_json::Result<()> {
    assert_upstream_prompt_round_trip("")
}

#[test]
fn nul_prompt_strings_remain_lossless_in_json() -> serde_json::Result<()> {
    assert_upstream_prompt_round_trip("before\0after")
}
