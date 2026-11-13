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
