use serde_json::json;
use trogon_claude_agent_sdk::{ClaudeAgentOptions, SettingSource};

/// Keep the source names compatible with the upstream literal unions.
#[test]
fn setting_sources_round_trip_upstream_literals() -> Result<(), serde_json::Error> {
    for (source, literal) in [
        (SettingSource::User, "user"),
        (SettingSource::Project, "project"),
        (SettingSource::Local, "local"),
    ] {
        assert_eq!(serde_json::to_value(source)?, json!(literal));
        assert_eq!(serde_json::from_value::<SettingSource>(json!(literal))?, source);
    }
    Ok(())
}

/// Reject names outside the exact upstream source union.
#[test]
fn setting_sources_reject_unsupported_values() {
    for value in [json!(""), json!("User"), json!("global"), json!(null), json!(1)] {
        assert!(serde_json::from_value::<SettingSource>(value).is_err());
    }
}

/// Preserve omission, isolation, caller order and repeated entries without local restrictions.
#[test]
fn setting_sources_preserve_optional_list_semantics() {
    assert_eq!(ClaudeAgentOptions::default().setting_sources, None);
    for sources in [
        vec![],
        vec![
            SettingSource::Local,
            SettingSource::User,
            SettingSource::Project,
            SettingSource::User,
        ],
    ] {
        let options = ClaudeAgentOptions {
            setting_sources: Some(sources.clone()),
            ..ClaudeAgentOptions::default()
        };
        assert_eq!(options.setting_sources, Some(sources));
    }
}
