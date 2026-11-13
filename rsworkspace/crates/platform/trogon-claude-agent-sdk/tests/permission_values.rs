#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use std::sync::Arc;

use serde_json::json;
use trogon_claude_agent_sdk::{
    CancellationToken, ControlRequestId, DenialDisposition, FeatureSupport, JsonObject, PermissionBehavior,
    PermissionDecision, PermissionDecisionClassification, PermissionMode, PermissionRuleValue, PermissionUpdate,
    PermissionUpdateDestination, ToolName, ToolPermissionHandler, ToolPermissionRequest,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request() -> ToolPermissionRequest {
    ToolPermissionRequest {
        tool_name: ToolName::from("Bash"),
        input: JsonObject::new(),
        tool_use_id: None,
        blocked_path: None,
        decision_reason: None,
        request_id: ControlRequestId::from("request-1"),
        signal: CancellationToken::new(),
        suggestions: Vec::new(),
        agent_id: None,
        mcp_server: None,
        title: None,
        display_name: None,
        description: None,
        default_to_no: FeatureSupport::NotReported,
        suppress_always_allow_rule: FeatureSupport::NotReported,
        requires_user_interaction: FeatureSupport::NotReported,
        decision_reason_type: None,
        decision_reason_code: None,
        server_policy_ask_only: FeatureSupport::NotReported,
        plugin: None,
        server_prompt: None,
        computer_folder: None,
        matched_ask_rule: None,
        additional_fields: JsonObject::new(),
        raw_request: JsonObject::new(),
    }
}

#[test]
fn decisions_default_to_unmodified_allow_and_continuing_deny() {
    assert!(matches!(
        PermissionDecision::allow(),
        PermissionDecision::Allow {
            updated_input: None,
            updated_permissions: None,
            decision_classification: None,
        }
    ));
    assert!(matches!(
        PermissionDecision::deny("no"),
        PermissionDecision::Deny {
            message,
            disposition: DenialDisposition::Continue,
            decision_classification: None,
        } if message == "no"
    ));
    assert_eq!(DenialDisposition::default(), DenialDisposition::Continue);
}

#[test]
fn decision_classifications_are_the_typescript_literals() {
    assert_eq!(
        PermissionDecisionClassification::UserTemporary.as_str(),
        "user_temporary"
    );
    assert_eq!(
        PermissionDecisionClassification::UserPermanent.as_str(),
        "user_permanent"
    );
    assert_eq!(PermissionDecisionClassification::UserReject.as_str(), "user_reject");
}

#[tokio::test]
async fn closures_are_permission_handlers_with_an_opaque_debug_form() {
    let handler: Arc<dyn ToolPermissionHandler> = Arc::new(|request: ToolPermissionRequest| async move {
        PermissionDecision::deny(request.tool_name.as_str().to_owned())
    });
    assert!(matches!(
        handler.decide(request()).await,
        PermissionDecision::Deny { message, .. } if message == "Bash"
    ));
    assert_eq!(format!("{:?}", &*handler), "ToolPermissionHandler { .. }");
}

#[test]
fn permission_modes_are_built_from_owned_strings() {
    for (literal, mode) in [
        ("default", PermissionMode::Default),
        ("acceptEdits", PermissionMode::AcceptEdits),
        ("plan", PermissionMode::Plan),
        ("bypassPermissions", PermissionMode::BypassPermissions),
        ("dontAsk", PermissionMode::DontAsk),
        ("auto", PermissionMode::Auto),
        ("custom", PermissionMode::Unknown("custom".into())),
    ] {
        assert_eq!(PermissionMode::from(literal.to_owned()), mode);
    }
}

fn rule(content: Option<&str>) -> PermissionRuleValue {
    PermissionRuleValue {
        tool_name: ToolName::from("Bash"),
        rule_content: content.map(str::to_owned),
    }
}

#[test]
fn every_permission_update_kind_round_trips_through_its_wire_form() -> TestResult {
    let destination = PermissionUpdateDestination::LocalSettings;
    let behavior = PermissionBehavior::Ask;
    let updates = [
        PermissionUpdate::AddRules {
            rules: vec![rule(Some("git"))],
            behavior: behavior.clone(),
            destination: destination.clone(),
        },
        PermissionUpdate::ReplaceRules {
            rules: vec![rule(None)],
            behavior: behavior.clone(),
            destination: destination.clone(),
        },
        PermissionUpdate::RemoveRules {
            rules: vec![rule(None), rule(Some("ls"))],
            behavior: behavior.clone(),
            destination: destination.clone(),
        },
        PermissionUpdate::SetMode {
            mode: PermissionMode::Unknown("custom".into()),
            destination: destination.clone(),
        },
        PermissionUpdate::SetMode {
            mode: PermissionMode::Plan,
            destination: destination.clone(),
        },
        PermissionUpdate::AddDirectories {
            directories: vec!["/a".into()],
            destination: destination.clone(),
        },
        PermissionUpdate::RemoveDirectories {
            directories: vec!["/b".into()],
            destination: destination.clone(),
        },
    ];
    for update in updates {
        let wire = update.to_wire_json()?;
        let decoded = PermissionUpdate::from_wire_json(serde_json::from_value(wire.clone())?)?;
        assert_eq!(decoded.to_wire_json()?, wire);
    }
    for mode in [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
        PermissionMode::BypassPermissions,
        PermissionMode::DontAsk,
        PermissionMode::Auto,
    ] {
        let wire = PermissionUpdate::SetMode {
            mode: mode.clone(),
            destination: destination.clone(),
        }
        .to_wire_json()?;
        assert_eq!(wire["mode"], serde_json::to_value(&mode)?);
    }
    assert_eq!(
        PermissionUpdate::SetMode {
            mode: PermissionMode::AcceptEdits,
            destination: destination.clone()
        }
        .to_wire_json()?,
        json!({"type": "setMode", "mode": "acceptEdits", "destination": "localSettings"})
    );
    Ok(())
}

#[test]
fn structurally_malformed_permission_updates_are_rejected() -> TestResult {
    for value in [
        json!({"type": "addRules", "destination": "session"}),
        json!({"type": "addRules", "rules": {}, "behavior": "allow", "destination": "session"}),
        json!({"destination": "session"}),
        json!({"type": 7, "destination": "session"}),
        json!({"type": "setMode", "mode": "plan", "destination": 7}),
        json!({"type": "addRules", "rules": [], "behavior": null, "destination": "session"}),
    ] {
        let error = PermissionUpdate::from_wire_json(serde_json::from_value(value)?);
        assert!(error.is_err());
    }
    Ok(())
}

#[test]
fn unknown_permission_update_values_are_preserved_verbatim() -> TestResult {
    let unknown_type = json!({"type": "future", "destination": "session", "extra": [1, {"a": null}]});
    let unknown_destination = json!({"type": "setMode", "mode": "plan", "destination": "elsewhere"});
    let unknown_behavior = json!({
        "type": "addRules",
        "rules": [{"toolName": "Bash", "ruleContent": "ls"}],
        "behavior": "prompt",
        "destination": "session",
    });
    for wire in [&unknown_type, &unknown_destination, &unknown_behavior] {
        let decoded = PermissionUpdate::from_wire_json(serde_json::from_value(wire.clone())?)?;
        assert_eq!(&decoded.to_wire_json()?, wire);
    }
    assert!(matches!(
        PermissionUpdate::from_wire_json(serde_json::from_value(unknown_type.clone())?)?,
        PermissionUpdate::Unknown(object) if serde_json::Value::Object(object.clone()) == unknown_type
    ));
    assert!(matches!(
        PermissionUpdate::from_wire_json(serde_json::from_value(unknown_destination)?)?,
        PermissionUpdate::SetMode { destination: PermissionUpdateDestination::Unknown(value), .. } if value == "elsewhere"
    ));
    assert!(matches!(
        PermissionUpdate::from_wire_json(serde_json::from_value(unknown_behavior)?)?,
        PermissionUpdate::AddRules { behavior: PermissionBehavior::Unknown(value), .. } if value == "prompt"
    ));
    Ok(())
}

#[test]
fn permission_update_destinations_and_behaviors_use_camel_case_and_lowercase() -> TestResult {
    for (destination, literal) in [
        (PermissionUpdateDestination::UserSettings, "userSettings"),
        (PermissionUpdateDestination::ProjectSettings, "projectSettings"),
        (PermissionUpdateDestination::Session, "session"),
        (PermissionUpdateDestination::CliArg, "cliArg"),
    ] {
        assert_eq!(serde_json::to_value(destination)?, json!(literal));
    }
    for (behavior, literal) in [
        (PermissionBehavior::Allow, "allow"),
        (PermissionBehavior::Deny, "deny"),
        (PermissionBehavior::Unknown("later".into()), "later"),
    ] {
        assert_eq!(serde_json::to_value(&behavior)?, json!(literal));
        assert_eq!(serde_json::from_value::<PermissionBehavior>(json!(literal))?, behavior);
    }
    assert_eq!(
        serde_json::from_value::<PermissionUpdateDestination>(json!("later"))?,
        PermissionUpdateDestination::Unknown("later".into())
    );
    Ok(())
}
