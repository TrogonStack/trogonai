use serde_json::json;
use trogon_claude_agent_sdk::{ClaudeAgentOptions, PermissionMode};

/// Preserve the literal spellings accepted by both pinned upstream SDKs.
#[test]
fn permission_modes_round_trip_upstream_literals() -> Result<(), serde_json::Error> {
    for (mode, literal) in [
        (PermissionMode::Default, "default"),
        (PermissionMode::AcceptEdits, "acceptEdits"),
        (PermissionMode::Plan, "plan"),
        (PermissionMode::BypassPermissions, "bypassPermissions"),
        (PermissionMode::DontAsk, "dontAsk"),
        (PermissionMode::Auto, "auto"),
    ] {
        assert_eq!(serde_json::to_value(&mode)?, json!(literal));
        assert_eq!(serde_json::from_value::<PermissionMode>(json!(literal))?, mode);

        let options = ClaudeAgentOptions {
            permission_mode: Some(mode.clone()),
            ..ClaudeAgentOptions::default()
        };
        assert_eq!(options.permission_mode, Some(mode));
    }
    Ok(())
}

/// An omitted mode and an explicit upstream default remain distinct choices.
#[test]
fn permission_mode_remains_unset_until_explicitly_selected() {
    assert_eq!(ClaudeAgentOptions::default().permission_mode, None);
    let options = ClaudeAgentOptions {
        permission_mode: Some(PermissionMode::Default),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(options.permission_mode, Some(PermissionMode::Default));
}

/// Both upstream SDKs pass the mode string through unvalidated, so an unknown spelling round-trips unchanged.
#[test]
fn s17_unknown_permission_modes_round_trip_unchanged() -> Result<(), serde_json::Error> {
    for literal in ["", "unknown", "Default", "accept_edits", "bypasspermissions"] {
        let mode = serde_json::from_value::<PermissionMode>(json!(literal))?;
        assert_eq!(mode, PermissionMode::Unknown(literal.into()));
        assert_eq!(serde_json::to_value(&mode)?, json!(literal));
    }
    Ok(())
}
