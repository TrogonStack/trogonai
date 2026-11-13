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
    CallbackError, CancellationToken, ControlRequestId, ElicitationHandler, ElicitationMode, ElicitationRequest,
    ElicitationResponse, HookHandler, HookOutput, HookRequest, JsonObject, McpServerName, UserDialogHandler,
    UserDialogRequest, UserDialogResponse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn closures_are_hook_handlers() -> TestResult {
    let handler: Arc<dyn HookHandler> = Arc::new(|request: HookRequest| async move {
        if request.tool_use_id.is_some() {
            Err(CallbackError::Cancelled)
        } else {
            Ok(HookOutput::block("blocked"))
        }
    });
    let request = HookRequest {
        input: JsonObject::new(),
        tool_use_id: None,
        signal: CancellationToken::new(),
    };
    let output = handler.call(request).await?;
    assert_eq!(
        output.to_wire_json()?,
        json!({"decision": "block", "reason": "blocked"})
    );
    Ok(())
}

#[tokio::test]
async fn closures_are_elicitation_handlers_with_an_opaque_debug_form() -> TestResult {
    let handler: Arc<dyn ElicitationHandler> =
        Arc::new(|_: ElicitationRequest| async move { Ok(ElicitationResponse::Decline) });
    let response = handler
        .call(ElicitationRequest {
            server_name: McpServerName::from("server"),
            message: "need input".into(),
            mode: Some(ElicitationMode::Form),
            url: None,
            elicitation_id: None,
            requested_schema: None,
            title: None,
            display_name: None,
            description: None,
            request_id: ControlRequestId::from("elicitation-1"),
            signal: CancellationToken::new(),
        })
        .await?;
    assert_eq!(response.to_wire_json()?, json!({"action": "decline"}));
    assert_eq!(format!("{:?}", &*handler), "ElicitationHandler { .. }");
    Ok(())
}

#[tokio::test]
async fn closures_are_user_dialog_handlers_with_an_opaque_debug_form() -> TestResult {
    let handler: Arc<dyn UserDialogHandler> = Arc::new(|request: UserDialogRequest| async move {
        Ok(UserDialogResponse::Completed {
            result: json!({"kind": request.dialog_kind.as_str()}),
        })
    });
    let response = handler
        .call(UserDialogRequest {
            dialog_kind: "confirm".into(),
            payload: JsonObject::new(),
            tool_use_id: None,
            request_id: ControlRequestId::from("dialog-1"),
            signal: CancellationToken::new(),
        })
        .await?;
    assert_eq!(
        response.to_wire_json()?,
        json!({"behavior": "completed", "result": {"kind": "confirm"}})
    );
    assert_eq!(format!("{:?}", &*handler), "UserDialogHandler { .. }");
    Ok(())
}

#[test]
fn elicitation_modes_round_trip_their_wire_names() {
    for (name, mode) in [
        ("form", ElicitationMode::Form),
        ("url", ElicitationMode::Url),
        ("future", ElicitationMode::Other("future".into())),
    ] {
        assert_eq!(ElicitationMode::from(name.to_owned()), mode);
        assert_eq!(mode.as_str(), name);
    }
}

#[test]
fn an_out_of_band_answer_is_the_null_the_typescript_handler_returns() -> TestResult {
    assert_eq!(
        ElicitationResponse::AnsweredOutOfBand.to_wire_json()?,
        serde_json::Value::Null
    );
    assert_eq!(
        UserDialogResponse::AnsweredOutOfBand.to_wire_json()?,
        serde_json::Value::Null
    );
    assert_eq!(
        UserDialogResponse::Cancelled.to_wire_json()?,
        json!({"behavior": "cancelled"})
    );
    Ok(())
}
