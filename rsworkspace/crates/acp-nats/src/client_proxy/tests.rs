use super::*;
use crate::agent::test_support::set_wire_json_response;
use agent_client_protocol::{
    Client, ContentBlock, ContentChunk, ReadTextFileResponse, RequestPermissionOutcome, RequestPermissionResponse,
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
async fn request_elicitation_publishes_to_correct_subject() {
    use agent_client_protocol::{
        ElicitationAction, ElicitationFormMode, ElicitationMode, ElicitationResponse, ElicitationSchema,
    };
    let nats = AdvancedMockNatsClient::new();
    let response = ElicitationResponse::new(ElicitationAction::Cancel);
    set_wire_json_response(&nats, "acp.session.s1.client.session.elicitation", &response);

    let p = proxy(nats.clone());
    let request = agent_client_protocol::ElicitationRequest::new(
        "s1".to_string(),
        ElicitationMode::Form(ElicitationFormMode::new(ElicitationSchema::new())),
        "Need input",
    );
    let result = p.request_elicitation(request).await;

    assert!(result.is_ok());
    assert!(matches!(result.unwrap().action, ElicitationAction::Cancel));
}

#[tokio::test]
async fn request_permission_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
    set_wire_json_response(&nats, "acp.session.s1.client.session.request_permission", &response);

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
    assert_eq!(nats.published_messages(), vec!["acp.session.s1.client.session.update"]);
}

#[tokio::test]
async fn read_text_file_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = ReadTextFileResponse::new("file contents");
    set_wire_json_response(&nats, "acp.session.s1.client.fs.read_text_file", &response);

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
    assert_eq!(result.unwrap_err().code, ErrorCode::InternalError);
}

#[tokio::test]
async fn write_text_file_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = agent_client_protocol::WriteTextFileResponse::default();
    set_wire_json_response(&nats, "acp.session.s1.client.fs.write_text_file", &response);

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
    set_wire_json_response(&nats, "acp.session.s1.client.terminal.create", &response);

    let p = proxy(nats.clone());
    let result = p.create_terminal(CreateTerminalRequest::new("s1", "echo")).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn terminal_output_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = TerminalOutputResponse::new("output", false);
    set_wire_json_response(&nats, "acp.session.s1.client.terminal.output", &response);

    let p = proxy(nats.clone());
    let result = p.terminal_output(TerminalOutputRequest::new("s1", "t1")).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn release_terminal_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = ReleaseTerminalResponse::default();
    set_wire_json_response(&nats, "acp.session.s1.client.terminal.release", &response);

    let p = proxy(nats.clone());
    let result = p.release_terminal(ReleaseTerminalRequest::new("s1", "t1")).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn kill_terminal_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = KillTerminalResponse::default();
    set_wire_json_response(&nats, "acp.session.s1.client.terminal.kill", &response);

    let p = proxy(nats.clone());
    let result = p.kill_terminal(KillTerminalRequest::new("s1", "t1")).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn wait_for_terminal_exit_publishes_to_correct_subject() {
    let nats = AdvancedMockNatsClient::new();
    let response = WaitForTerminalExitResponse::new(agent_client_protocol::TerminalExitStatus::new());
    set_wire_json_response(&nats, "acp.session.s1.client.terminal.wait_for_exit", &response);

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
    assert_eq!(result.unwrap_err().code, ErrorCode::InternalError);
}

#[test]
fn to_acp_error_preserves_message() {
    let err = to_acp_error("something went wrong");
    assert_eq!(err.code, ErrorCode::InternalError);
    assert_eq!(err.message, "something went wrong");
}
