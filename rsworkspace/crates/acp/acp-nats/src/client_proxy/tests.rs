use super::*;
use crate::agent::test_support::set_wire_json_response;
use agent_client_protocol::v1::schema::{
    ContentBlock, ContentChunk, ElicitationAcceptAction, ElicitationFormMode, ElicitationSchema,
    ElicitationSessionScope, ReadTextFileResponse, RequestPermissionOutcome, RequestPermissionResponse,
    SessionNotification, SessionUpdate, ToolCallUpdate, ToolCallUpdateFields,
};
use trogon_nats::AdvancedMockNatsClient;

fn proxy(nats: AdvancedMockNatsClient) -> NatsClientProxy<AdvancedMockNatsClient> {
    NatsClientProxy::new(
        nats,
        AcpSessionId::new("s1").unwrap(),
        AcpPrefix::new("acp").unwrap(),
        Duration::from_secs(5),
    )
}

#[tokio::test]
async fn request_permission_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
    set_wire_json_response(&nats, "acp.v1.session.s1.client.session.request_permission", &response);

    let p = proxy(nats.clone());
    let tool_call = ToolCallUpdate::new("tc-1", ToolCallUpdateFields::new());
    let result = p
        .request_permission(RequestPermissionRequest::new("s1", tool_call, vec![]))
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().outcome, RequestPermissionOutcome::Cancelled);
}

#[tokio::test]
async fn session_notification_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let p = proxy(nats.clone());

    let notif = SessionNotification::new(
        "s1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
    );
    let result = p.session_notification(notif).await;

    assert!(result.is_ok());
    assert_eq!(
        nats.published_messages(),
        vec!["acp.v1.session.s1.client.session.update"]
    );
}

#[tokio::test]
async fn read_text_file_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = ReadTextFileResponse::new("file contents");
    set_wire_json_response(&nats, "acp.v1.session.s1.client.fs.read_text_file", &response);

    let p = proxy(nats.clone());
    let result = p.read_text_file(ReadTextFileRequest::new("s1", "/test.txt")).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().content, "file contents");
}

#[tokio::test]
async fn request_returns_error_when_nats_fails() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();

    let p = proxy(nats);
    let result = p.read_text_file(ReadTextFileRequest::new("s1", "/test.txt")).await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::INTERNAL_ERROR);
}

#[tokio::test]
async fn write_text_file_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = agent_client_protocol::v1::schema::WriteTextFileResponse::default();
    set_wire_json_response(&nats, "acp.v1.session.s1.client.fs.write_text_file", &response);

    let p = proxy(nats.clone());
    let result = p
        .write_text_file(WriteTextFileRequest::new("s1", "/test.txt", "content"))
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn create_terminal_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = CreateTerminalResponse::new("t1");
    set_wire_json_response(&nats, "acp.v1.session.s1.client.terminal.create", &response);

    let p = proxy(nats.clone());
    let result = p.create_terminal(CreateTerminalRequest::new("s1", "echo")).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn terminal_output_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = TerminalOutputResponse::new("output", false);
    set_wire_json_response(&nats, "acp.v1.session.s1.client.terminal.output", &response);

    let p = proxy(nats.clone());
    let result = p.terminal_output(TerminalOutputRequest::new("s1", "t1")).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn release_terminal_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = ReleaseTerminalResponse::default();
    set_wire_json_response(&nats, "acp.v1.session.s1.client.terminal.release", &response);

    let p = proxy(nats.clone());
    let result = p.release_terminal(ReleaseTerminalRequest::new("s1", "t1")).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn kill_terminal_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = KillTerminalResponse::default();
    set_wire_json_response(&nats, "acp.v1.session.s1.client.terminal.kill", &response);

    let p = proxy(nats.clone());
    let result = p.kill_terminal(KillTerminalRequest::new("s1", "t1")).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn wait_for_terminal_exit_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = WaitForTerminalExitResponse::new(agent_client_protocol::v1::schema::TerminalExitStatus::new());
    set_wire_json_response(&nats, "acp.v1.session.s1.client.terminal.wait_for_exit", &response);

    let p = proxy(nats.clone());
    let result = p
        .wait_for_terminal_exit(WaitForTerminalExitRequest::new("s1", "t1"))
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn notification_returns_error_when_publish_fails() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();

    let p = proxy(nats);
    let notif = SessionNotification::new(
        "s1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
    );
    let result = p.session_notification(notif).await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::INTERNAL_ERROR);
}

#[tokio::test]
async fn elicitation_create_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = CreateElicitationResponse::new(ElicitationAcceptAction::new());
    set_wire_json_response(&nats, "acp.v1.session.s1.client.elicitation.create", &response);

    let p = proxy(nats.clone());
    let mode = ElicitationFormMode::new(ElicitationSessionScope::new("s1"), ElicitationSchema::new());
    let result = p
        .elicitation_create(CreateElicitationRequest::new(mode, "please respond"))
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn elicitation_complete_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let p = proxy(nats.clone());

    let result = p
        .elicitation_complete(CompleteElicitationNotification::new("elicitation-1"))
        .await;

    assert!(result.is_ok());
    assert_eq!(
        nats.published_messages(),
        vec!["acp.v1.session.s1.client.elicitation.complete"]
    );
}

#[test]
fn to_acp_error_preserves_message() {
    let err = to_acp_error("something went wrong");
    assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
    assert_eq!(err.message, "something went wrong");
}

fn raw(json: &str) -> std::sync::Arc<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string(json.to_string())
        .unwrap()
        .into()
}

#[tokio::test]
async fn ext_method_requests_on_the_session_ext_subject_with_the_exact_name() {
    let nats = AdvancedMockNatsClient::new();
    set_wire_json_response(
        &nats,
        "acp.v1.session.s1.client.ext._vendor/x",
        &serde_json::json!({"ok": true}),
    );

    let p = proxy(nats.clone());
    let response = p
        .ext_method(ExtRequest::new("_vendor/x", raw(r#"{"sessionId":"s1"}"#)))
        .await
        .unwrap();

    assert_eq!(response.0.get(), r#"{"ok":true}"#);
    let body: serde_json::Value = serde_json::from_slice(&nats.requested_payloads()[0]).unwrap();
    assert_eq!(body["params"], serde_json::json!({"sessionId": "s1"}));
}

#[tokio::test]
async fn ext_method_adds_the_leading_underscore_only_when_missing() {
    let nats = AdvancedMockNatsClient::new();
    set_wire_json_response(
        &nats,
        "acp.v1.session.s1.client.ext._vendor/x",
        &serde_json::json!(null),
    );

    let p = proxy(nats.clone());
    p.ext_method(ExtRequest::new("vendor/x", raw("{}"))).await.unwrap();
}

#[tokio::test]
async fn ext_method_refuses_a_name_no_subject_can_carry() {
    let p = proxy(AdvancedMockNatsClient::new());
    let error = p.ext_method(ExtRequest::new("_bad name", raw("{}"))).await.unwrap_err();
    assert_eq!(error.code, agent_client_protocol::ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn ext_notification_publishes_on_the_session_ext_subject() {
    let nats = AdvancedMockNatsClient::new();
    let p = proxy(nats.clone());

    p.ext_notification(ExtNotification::new("_vendor/x", raw(r#"{"sessionId":"s1"}"#)))
        .await
        .unwrap();

    assert_eq!(
        nats.published_messages(),
        vec!["acp.v1.session.s1.client.ext._vendor/x"]
    );
}
