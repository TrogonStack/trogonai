use super::*;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
    RequestPermissionOutcome, SessionUpdate, ToolCallUpdate, ToolCallUpdateFields,
};
use std::sync::Arc;

struct MinimalClient;

#[async_trait::async_trait]
impl ClientHandler for MinimalClient {
    async fn request_permission(&self, _args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn session_notification(&self, _args: SessionNotification) -> Result<()> {
        Ok(())
    }
}

fn assert_method_not_found<T>(result: Result<T>) {
    let error = result.err().unwrap();
    assert_eq!(error.code, ErrorCode::MethodNotFound);
}

fn empty_raw_value() -> Arc<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string("{}".to_string())
        .unwrap()
        .into()
}

#[tokio::test]
async fn required_methods_are_callable() {
    let client = MinimalClient;

    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let permission = client
        .request_permission(RequestPermissionRequest::new("s1", tool_call, vec![]))
        .await;
    assert!(permission.is_ok());

    let notification = SessionNotification::new(
        "s1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
    );
    assert!(client.session_notification(notification).await.is_ok());
}

#[tokio::test]
async fn optional_methods_default_to_method_not_found() {
    let client = MinimalClient;

    assert_method_not_found(client.read_text_file(ReadTextFileRequest::new("s1", "/tmp/f")).await);
    assert_method_not_found(
        client
            .write_text_file(WriteTextFileRequest::new("s1", "/tmp/f", "content"))
            .await,
    );
    assert_method_not_found(client.create_terminal(CreateTerminalRequest::new("s1", "echo")).await);
    assert_method_not_found(client.terminal_output(TerminalOutputRequest::new("s1", "t1")).await);
    assert_method_not_found(client.release_terminal(ReleaseTerminalRequest::new("s1", "t1")).await);
    assert_method_not_found(
        client
            .wait_for_terminal_exit(WaitForTerminalExitRequest::new("s1", "t1"))
            .await,
    );
    assert_method_not_found(client.kill_terminal(KillTerminalRequest::new("s1", "t1")).await);
    assert_method_not_found(client.ext_method(ExtRequest::new("ext", empty_raw_value())).await);
    assert_method_not_found(
        client
            .ext_notification(ExtNotification::new("ext", empty_raw_value()))
            .await,
    );

    let mode = ElicitationFormMode::new(ElicitationSessionScope::new("s1"), ElicitationSchema::new());
    assert_method_not_found(
        client
            .elicitation_create(CreateElicitationRequest::new(mode, "please respond"))
            .await,
    );
    assert_method_not_found(
        client
            .elicitation_complete(CompleteElicitationNotification::new("elicitation-1"))
            .await,
    );
}
