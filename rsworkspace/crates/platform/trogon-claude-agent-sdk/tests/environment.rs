use std::collections::BTreeMap;
use trogon_claude_agent_sdk::ClaudeAgentOptions;

#[test]
fn environment_overrides_are_owned_and_preserve_empty_values() {
    assert!(ClaudeAgentOptions::default().env.is_empty());
    let options = ClaudeAgentOptions {
        env: BTreeMap::from([
            ("SDK_FIXTURE_SETTING".into(), "fixture-value".into()),
            ("SDK_FIXTURE_EMPTY".into(), String::new()),
        ]),
        ..ClaudeAgentOptions::default()
    };
    let mut cloned = options.clone();
    cloned.env.insert("SDK_FIXTURE_SETTING".into(), "changed".into());
    assert_eq!(
        options.env.get("SDK_FIXTURE_SETTING").map(String::as_str),
        Some("fixture-value")
    );
    assert_eq!(options.env.get("SDK_FIXTURE_EMPTY").map(String::as_str), Some(""));
    assert_eq!(
        cloned.env.get("SDK_FIXTURE_SETTING").map(String::as_str),
        Some("changed")
    );
}

#[test]
fn options_debug_does_not_disclose_environment_entries_or_model_strings() {
    let options = ClaudeAgentOptions {
        model: Some("fixture-model-canary".into()),
        env: BTreeMap::from([(
            "FIXTURE_ENVIRONMENT_KEY".into(),
            "fixture-environment-value-canary".into(),
        )]),
        ..ClaudeAgentOptions::default()
    };
    let debug = format!("{options:?}");
    assert!(debug.starts_with("ClaudeAgentOptions {"));
    for value in [
        "FIXTURE_ENVIRONMENT_KEY",
        "fixture-environment-value-canary",
        "fixture-model-canary",
    ] {
        assert!(!debug.contains(value));
    }
}

#[test]
fn options_debug_shows_non_sensitive_settings() {
    let options = ClaudeAgentOptions {
        cwd: Some("fixture-cwd".into()),
        max_turns: std::num::NonZeroUsize::new(7),
        ..ClaudeAgentOptions::default()
    };
    let debug = format!("{options:?}");
    for field in [
        "cwd: Some(\"fixture-cwd\")",
        "max_turns: Some(7)",
        "setting_sources",
        "max_budget_usd",
        "effort",
    ] {
        assert!(debug.contains(field), "{debug}");
    }
}
