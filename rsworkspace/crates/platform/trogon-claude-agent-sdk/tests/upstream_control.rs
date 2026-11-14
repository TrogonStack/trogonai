//! Round trips every control wire type the pinned declarations list through the generated serde types, with the
//! samples `synthesize_fixtures.mjs` writes from those declarations.

use std::collections::BTreeSet;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use trogon_claude_agent_sdk::js_stringify;
use trogon_claude_agent_sdk::upstream::control::{
    ControlErrorResponse, ControlResponse, SDKControlApplyFlagSettingsRequest, SDKControlBackgroundTasksRequest,
    SDKControlCancelAsyncMessageRequest, SDKControlCancelRequest, SDKControlElicitationRequest,
    SDKControlFileSuggestionsRequest, SDKControlGetBinaryVersionRequest, SDKControlGetContextUsageRequest,
    SDKControlGetHooksListingRequest, SDKControlGetSessionCostRequest, SDKControlGetSettingsRequest,
    SDKControlGetTaskOutputRequest, SDKControlGetUsageRequest, SDKControlInitializeRequest, SDKControlInterruptRequest,
    SDKControlListModelsRequest, SDKControlListPermissionRulesRequest, SDKControlMcpCallRequest,
    SDKControlMcpMessageRequest, SDKControlMcpReadResourceRequest, SDKControlMcpReconnectRequest,
    SDKControlMcpSetServersRequest, SDKControlMcpStatusRequest, SDKControlMcpToggleRequest,
    SDKControlPermissionRequest, SDKControlReadFileRequest, SDKControlRegisterRepoRootRequest,
    SDKControlReloadOutputStylesRequest, SDKControlReloadPluginsRequest, SDKControlReloadSkillsRequest,
    SDKControlRenameSessionRequest, SDKControlRequest, SDKControlRequestUserDialogRequest, SDKControlResponse,
    SDKControlRewindFilesRequest, SDKControlSeedReadStateRequest, SDKControlSetColorRequest,
    SDKControlSetHasSubscribersRequest, SDKControlSetMaxThinkingTokensRequest, SDKControlSetModelRequest,
    SDKControlSetPermissionModeRequest, SDKControlStopTaskRequest, SDKControlUpdateSettingsRequest,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURES: &str = include_str!("fixtures/upstream_control.json");

fn samples(root: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut fixtures: serde_json::Map<String, Value> = serde_json::from_str(FIXTURES)?;
    match fixtures.remove(root) {
        Some(Value::Array(samples)) if !samples.is_empty() => Ok(samples),
        _ => Err(format!("fixture root {root} is missing").into()),
    }
}

/// Decoding then encoding every sample gives the sample back, key order included.
fn round_trips<T: Serialize + DeserializeOwned>(root: &str) -> TestResult {
    for sample in samples(root)? {
        let typed: T = serde_json::from_value(sample.clone())?;
        let encoded = serde_json::to_value(&typed)?;
        assert_eq!(
            js_stringify(&encoded),
            js_stringify(&sample),
            "{root} did not round-trip"
        );
        let text = serde_json::to_string(&sample)?;
        let reparsed: T = serde_json::from_str(&text)?;
        assert_eq!(
            serde_json::to_string(&reparsed)?,
            text,
            "{root} did not re-encode byte-identically"
        );
    }
    Ok(())
}

macro_rules! round_trip_roots {
    ($($root:ident),+ $(,)?) => {
        const ROOTS: &[&str] = &[$(stringify!($root)),+];

        #[test]
        fn every_control_wire_type_round_trips() -> TestResult {
            $(round_trips::<$root>(stringify!($root))?;)+
            Ok(())
        }
    };
}

round_trip_roots!(
    SDKControlRequest,
    SDKControlResponse,
    ControlResponse,
    ControlErrorResponse,
    SDKControlApplyFlagSettingsRequest,
    SDKControlBackgroundTasksRequest,
    SDKControlCancelAsyncMessageRequest,
    SDKControlCancelRequest,
    SDKControlElicitationRequest,
    SDKControlFileSuggestionsRequest,
    SDKControlGetBinaryVersionRequest,
    SDKControlGetContextUsageRequest,
    SDKControlGetHooksListingRequest,
    SDKControlGetSessionCostRequest,
    SDKControlGetSettingsRequest,
    SDKControlGetTaskOutputRequest,
    SDKControlGetUsageRequest,
    SDKControlInitializeRequest,
    SDKControlInterruptRequest,
    SDKControlListModelsRequest,
    SDKControlListPermissionRulesRequest,
    SDKControlMcpCallRequest,
    SDKControlMcpMessageRequest,
    SDKControlMcpReadResourceRequest,
    SDKControlMcpReconnectRequest,
    SDKControlMcpSetServersRequest,
    SDKControlMcpStatusRequest,
    SDKControlMcpToggleRequest,
    SDKControlPermissionRequest,
    SDKControlReadFileRequest,
    SDKControlRegisterRepoRootRequest,
    SDKControlReloadOutputStylesRequest,
    SDKControlReloadPluginsRequest,
    SDKControlReloadSkillsRequest,
    SDKControlRenameSessionRequest,
    SDKControlRequestUserDialogRequest,
    SDKControlRewindFilesRequest,
    SDKControlSeedReadStateRequest,
    SDKControlSetColorRequest,
    SDKControlSetHasSubscribersRequest,
    SDKControlSetMaxThinkingTokensRequest,
    SDKControlSetModelRequest,
    SDKControlSetPermissionModeRequest,
    SDKControlStopTaskRequest,
    SDKControlUpdateSettingsRequest,
);

#[test]
fn the_fixtures_hold_exactly_the_listed_roots() -> TestResult {
    let fixtures: serde_json::Map<String, Value> = serde_json::from_str(FIXTURES)?;
    let recorded: BTreeSet<&str> = fixtures.keys().map(String::as_str).collect();
    let listed: BTreeSet<&str> = ROOTS.iter().copied().collect();
    assert_eq!(recorded, listed);
    assert_eq!(ROOTS.len(), listed.len(), "a root is listed twice");
    Ok(())
}

#[test]
fn every_request_type_is_its_own_member_of_the_request_union() -> TestResult {
    let mut checked = 0;
    for root in ROOTS {
        if !root.starts_with("SDKControl") || !root.ends_with("Request") {
            continue;
        }
        if matches!(*root, "SDKControlRequest" | "SDKControlCancelRequest") {
            continue;
        }
        for sample in samples(root)? {
            let envelope = json!({"type": "control_request", "request_id": "r", "request": sample});
            let typed: SDKControlRequest = serde_json::from_value(envelope.clone())?;
            assert!(
                format!("{:?}", typed.request).starts_with(&format!("{root}(")),
                "{root} decoded as {:?}",
                typed.request
            );
            assert_eq!(
                js_stringify(&serde_json::to_value(&typed)?),
                js_stringify(&envelope),
                "{root} did not round-trip in its envelope"
            );
            checked += 1;
        }
    }
    assert!(checked >= 38, "only {checked} request samples were checked");
    Ok(())
}
