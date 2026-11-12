use trogon_claude_agent_sdk::{ClaudeAgentOptions, ConfigValue, Environment, SensitiveString};

#[test]
fn environment_overrides_are_owned_and_preserve_empty_values() {
    assert!(ClaudeAgentOptions::default().env.is_empty());
    let options = ClaudeAgentOptions {
        env: Environment::from([
            ("SDK_FIXTURE_SETTING".into(), "fixture-value".into()),
            ("SDK_FIXTURE_EMPTY".into(), String::new().into()),
        ]),
        ..ClaudeAgentOptions::default()
    };
    let mut cloned = options.clone();
    cloned.env.insert("SDK_FIXTURE_SETTING".into(), "changed".into());
    assert_eq!(value(&options.env, "SDK_FIXTURE_SETTING"), Some("fixture-value"));
    assert_eq!(value(&options.env, "SDK_FIXTURE_EMPTY"), Some(""));
    assert_eq!(value(&cloned.env, "SDK_FIXTURE_SETTING"), Some("changed"));
}

#[test]
fn environment_values_are_sensitive_unless_marked_plain() {
    assert!(ConfigValue::from("fixture-value").is_sensitive());
    assert!(ConfigValue::from(String::from("fixture-value")).is_sensitive());
    assert!(!ConfigValue::Plain("fixture-value".into()).is_sensitive());
    assert_eq!(SensitiveString::from("fixture-value").expose(), "fixture-value");
}

#[test]
fn options_debug_shows_environment_names_and_plain_values_only() {
    let options = ClaudeAgentOptions {
        model: Some("fixture-model-canary".into()),
        env: Environment::from([
            ("FIXTURE_SECRET_KEY".into(), "fixture-secret-value-canary".into()),
            (
                "FIXTURE_PLAIN_KEY".into(),
                ConfigValue::Plain("fixture-plain-value".into()),
            ),
        ]),
        ..ClaudeAgentOptions::default()
    };
    let debug = format!("{options:?}");
    assert!(debug.starts_with("ClaudeAgentOptions {"));
    for shown in [
        "FIXTURE_SECRET_KEY",
        "FIXTURE_PLAIN_KEY",
        "fixture-plain-value",
        "<redacted>",
    ] {
        assert!(debug.contains(shown), "{debug}");
    }
    for hidden in ["fixture-secret-value-canary", "fixture-model-canary"] {
        assert!(!debug.contains(hidden), "{debug}");
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

#[test]
fn environment_debug_names_every_variable_and_redacts_sensitive_values() {
    let environment = Environment::from([
        ("FIXTURE_KEY".into(), "fixture-value".into()),
        ("FIXTURE_PLAIN".into(), ConfigValue::Plain("shown".into())),
    ]);
    assert_eq!(
        format!("{environment:?}"),
        r#"Environment({"FIXTURE_KEY": Sensitive(<redacted>), "FIXTURE_PLAIN": Plain("shown")})"#
    );
    assert_eq!(
        environment
            .iter()
            .map(|(key, value)| (key, value.expose()))
            .collect::<Vec<_>>(),
        [("FIXTURE_KEY", "fixture-value"), ("FIXTURE_PLAIN", "shown")]
    );
}

fn value<'a>(environment: &'a Environment, key: &str) -> Option<&'a str> {
    environment.get(key).map(ConfigValue::expose)
}
