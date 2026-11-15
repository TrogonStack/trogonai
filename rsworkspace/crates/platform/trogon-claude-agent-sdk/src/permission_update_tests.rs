#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;
use serde_json::json;
#[test]
fn permission_updates_round_trip_and_reject_partial_rules() -> Result<(), PermissionUpdateError> {
    let update = PermissionUpdate::AddRules {
        rules: vec![PermissionRuleValue {
            tool_name: ToolName::from("Bash"),
            rule_content: Some("git status".into()),
        }],
        behavior: PermissionBehavior::Allow,
        destination: PermissionUpdateDestination::Session,
    };
    let wire = update.to_wire_json()?;
    let decoded = PermissionUpdate::from_wire_json(serde_json::from_value(wire.clone())?)?;
    assert_eq!(decoded.to_wire_json()?, wire);
    assert!(
        PermissionUpdate::from_wire_json(serde_json::from_value(
            json!({"type":"addRules","destination":"session"})
        )?)
        .is_err()
    );
    Ok(())
}

/// Python's `PermissionUpdate.to_dict()` output, captured from `claude_agent_sdk` 0.2.181 with compact separators.
#[test]
fn python_updates_are_written_in_to_dict_order() -> Result<(), serde_json::Error> {
    let rule = |tool: &str, content: Option<&str>| PermissionRuleValue {
        tool_name: ToolName::from(tool),
        rule_content: content.map(str::to_owned),
    };
    let cases = [
        (
            PermissionUpdate::AddRules {
                rules: vec![rule("Bash", Some("git status")), rule("Read", None)],
                behavior: PermissionBehavior::Allow,
                destination: PermissionUpdateDestination::UserSettings,
            },
            r#"{"type":"addRules","destination":"userSettings","rules":[{"toolName":"Bash","ruleContent":"git status"},{"toolName":"Read","ruleContent":null}],"behavior":"allow"}"#,
        ),
        (
            PermissionUpdate::ReplaceRules {
                rules: vec![rule("Edit", None)],
                behavior: PermissionBehavior::Deny,
                destination: PermissionUpdateDestination::ProjectSettings,
            },
            r#"{"type":"replaceRules","destination":"projectSettings","rules":[{"toolName":"Edit","ruleContent":null}],"behavior":"deny"}"#,
        ),
        (
            PermissionUpdate::RemoveRules {
                rules: vec![rule("Write", Some("src/**"))],
                behavior: PermissionBehavior::Ask,
                destination: PermissionUpdateDestination::LocalSettings,
            },
            r#"{"type":"removeRules","destination":"localSettings","rules":[{"toolName":"Write","ruleContent":"src/**"}],"behavior":"ask"}"#,
        ),
        (
            PermissionUpdate::AddDirectories {
                directories: vec!["/a".into(), "b".into()],
                destination: PermissionUpdateDestination::Session,
            },
            r#"{"type":"addDirectories","destination":"session","directories":["/a","b"]}"#,
        ),
        (
            PermissionUpdate::RemoveDirectories {
                directories: vec!["c".into()],
                destination: PermissionUpdateDestination::CliArg,
            },
            r#"{"type":"removeDirectories","destination":"cliArg","directories":["c"]}"#,
        ),
    ];
    for (update, expected) in cases {
        assert_eq!(serde_json::to_string(&update.to_python_wire_json()?)?, expected);
    }
    let raw: JsonObject = serde_json::from_value(json!({"type":"futureUpdate","extra":1}))?;
    assert_eq!(
        PermissionUpdate::Unknown(raw.clone()).to_python_wire_json()?,
        Value::Object(raw)
    );
    Ok(())
}
