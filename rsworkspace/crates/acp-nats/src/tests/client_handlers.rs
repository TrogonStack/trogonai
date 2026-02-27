use crate::agent::Bridge;
use crate::client::ext_session_prompt_response;
use crate::client::session_update;
use crate::config::Config;
use crate::tests::{MockNatsClient, noop_meter};
use agent_client_protocol::{
    Client, ContentBlock, ContentChunk, CreateTerminalRequest, CreateTerminalResponse, Error,
    KillTerminalCommandRequest, KillTerminalCommandResponse, PromptResponse, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate, StopReason, TerminalExitStatus, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    WriteTextFileRequest, WriteTextFileResponse,
};
use std::cell::RefCell;
use trogon_std::time::MockClock;

// --- ext_session_prompt_response ---

#[tokio::test]
async fn ext_session_prompt_response_resolves_waiter() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let session_id: agent_client_protocol::SessionId = "prompt-resp-001".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let response = PromptResponse::new(StopReason::EndTurn);
    let payload = serde_json::to_vec(&response).unwrap();

    ext_session_prompt_response::handle("prompt-resp-001", &payload, &bridge).await;

    let result = rx
        .await
        .expect("Should receive response")
        .expect("Prompt response should not include error");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn ext_session_prompt_response_no_waiter_does_not_panic() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );

    let response = PromptResponse::new(StopReason::EndTurn);
    let payload = serde_json::to_vec(&response).unwrap();

    ext_session_prompt_response::handle("no-waiter-session", &payload, &bridge).await;
}

#[tokio::test]
async fn ext_session_prompt_response_invalid_payload_removes_waiter() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let session_id: agent_client_protocol::SessionId = "bad-payload-001".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    ext_session_prompt_response::handle("bad-payload-001", b"not json", &bridge).await;

    let result = rx
        .await
        .expect("Should receive resolved parse error")
        .err()
        .expect("Parse failure should be forwarded to waiter");
    assert!(!result.is_empty(), "Expected parse error to be forwarded");
}

#[tokio::test]
async fn ext_session_prompt_response_invalid_session_id_is_rejected() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let session_id: agent_client_protocol::SessionId = "valid-session".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let response = PromptResponse::new(StopReason::EndTurn);
    let payload = serde_json::to_vec(&response).unwrap();

    // Session ID with dots (invalid for NATS subject token)
    ext_session_prompt_response::handle("session.with.dots", &payload, &bridge).await;

    // Session ID with wildcard
    ext_session_prompt_response::handle("session*wild", &payload, &bridge).await;

    // Session ID with whitespace
    ext_session_prompt_response::handle("session id", &payload, &bridge).await;

    assert!(
        bridge
            .pending_session_prompt_responses
            .has_waiter(&session_id),
        "invalid session IDs should not resolve valid waiter",
    );

    // Remove the waiter for the valid session explicitly.
    bridge
        .pending_session_prompt_responses
        .remove_waiter(&session_id);
    assert!(
        !bridge
            .pending_session_prompt_responses
            .has_waiter(&session_id),
        "waiter should be removed"
    );
    drop(rx);
}

// --- session_update ---

#[derive(Debug)]
struct MockClient {
    notifications_received: RefCell<Vec<String>>,
    should_fail: bool,
}

impl MockClient {
    fn new() -> Self {
        Self {
            notifications_received: RefCell::new(Vec::new()),
            should_fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            notifications_received: RefCell::new(Vec::new()),
            should_fail: true,
        }
    }

    fn notification_count(&self) -> usize {
        self.notifications_received.borrow().len()
    }
}

#[async_trait::async_trait(?Send)]
impl Client for MockClient {
    async fn session_notification(&self, notification: SessionNotification) -> Result<(), Error> {
        if self.should_fail {
            return Err(Error::new(-1, "mock failure"));
        }
        self.notifications_received
            .borrow_mut()
            .push(format!("{:?}", notification));
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, Error> {
        unimplemented!()
    }
}

#[tokio::test]
async fn session_update_forwards_notification_to_client() {
    let client = MockClient::new();
    let notification = SessionNotification::new(
        "session-001",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
    );
    let payload = serde_json::to_vec(&notification).unwrap();

    session_update::handle(&payload, &client).await;

    assert_eq!(client.notification_count(), 1);
}

#[tokio::test]
async fn session_update_invalid_payload_does_not_panic() {
    let client = MockClient::new();
    session_update::handle(b"not json", &client).await;
    assert_eq!(client.notification_count(), 0);
}

#[tokio::test]
async fn session_update_client_error_does_not_panic() {
    let client = MockClient::failing();
    let notification = SessionNotification::new(
        "session-001",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
    );
    let payload = serde_json::to_vec(&notification).unwrap();

    session_update::handle(&payload, &client).await;
}

// --- individual client RPC handlers ---

use crate::client::{
    fs_read_text_file, fs_write_text_file, request_permission, terminal_create, terminal_kill,
    terminal_output, terminal_release, terminal_wait_for_exit,
};

#[derive(Debug)]
struct RpcMockClient;

#[async_trait::async_trait(?Send)]
impl Client for RpcMockClient {
    async fn session_notification(&self, _: SessionNotification) -> Result<(), Error> {
        Ok(())
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> Result<ReadTextFileResponse, Error> {
        Ok(ReadTextFileResponse::new("file contents"))
    }

    async fn write_text_file(
        &self,
        _: WriteTextFileRequest,
    ) -> Result<WriteTextFileResponse, Error> {
        Ok(WriteTextFileResponse::new())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, Error> {
        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ))
    }

    async fn create_terminal(
        &self,
        _: CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, Error> {
        Ok(CreateTerminalResponse::new("term-001"))
    }

    async fn kill_terminal_command(
        &self,
        _: KillTerminalCommandRequest,
    ) -> Result<KillTerminalCommandResponse, Error> {
        Ok(KillTerminalCommandResponse::new())
    }

    async fn terminal_output(
        &self,
        _: TerminalOutputRequest,
    ) -> Result<TerminalOutputResponse, Error> {
        Ok(TerminalOutputResponse::new("output data", false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> Result<ReleaseTerminalResponse, Error> {
        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> Result<WaitForTerminalExitResponse, Error> {
        Ok(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(0u32),
        ))
    }
}

#[tokio::test]
async fn fs_read_text_file_round_trip() {
    let client = RpcMockClient;
    let request = ReadTextFileRequest::new("session-001", "/tmp/test.txt");
    let payload = serde_json::to_vec(&request).unwrap();

    let result = fs_read_text_file::handle(&payload, &client, 1_048_576).await;
    assert!(result.is_ok());

    let response: ReadTextFileResponse = serde_json::from_slice(&result.unwrap()).unwrap();
    assert_eq!(response.content, "file contents");
}

#[tokio::test]
async fn fs_write_text_file_round_trip() {
    let client = RpcMockClient;
    let request = WriteTextFileRequest::new("session-001", "/tmp/test.txt", "content");
    let payload = serde_json::to_vec(&request).unwrap();

    let result = fs_write_text_file::handle(&payload, &client, 1_048_576).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn terminal_create_round_trip() {
    let client = RpcMockClient;
    let request = CreateTerminalRequest::new("session-001", "echo hello");
    let payload = serde_json::to_vec(&request).unwrap();

    let result = terminal_create::handle(&payload, &client).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn terminal_kill_round_trip() {
    let client = RpcMockClient;
    let request = KillTerminalCommandRequest::new("session-001", "term-001");
    let payload = serde_json::to_vec(&request).unwrap();

    let result = terminal_kill::handle(&payload, &client).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn terminal_output_round_trip() {
    let client = RpcMockClient;
    let request = TerminalOutputRequest::new("session-001", "term-001");
    let payload = serde_json::to_vec(&request).unwrap();

    let result = terminal_output::handle(&payload, &client).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn terminal_release_round_trip() {
    let client = RpcMockClient;
    let request = ReleaseTerminalRequest::new("session-001", "term-001");
    let payload = serde_json::to_vec(&request).unwrap();

    let result = terminal_release::handle(&payload, &client).await;
    assert!(result.is_ok());
}

#[derive(Debug)]
struct SlowTerminalClient;

#[async_trait::async_trait(?Send)]
impl Client for SlowTerminalClient {
    async fn session_notification(&self, _: SessionNotification) -> Result<(), Error> {
        Ok(())
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> Result<ReadTextFileResponse, Error> {
        Ok(ReadTextFileResponse::new(""))
    }

    async fn write_text_file(
        &self,
        _: WriteTextFileRequest,
    ) -> Result<WriteTextFileResponse, Error> {
        Ok(WriteTextFileResponse::new())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, Error> {
        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ))
    }

    async fn create_terminal(
        &self,
        _: CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, Error> {
        Ok(CreateTerminalResponse::new("term-001"))
    }

    async fn kill_terminal_command(
        &self,
        _: KillTerminalCommandRequest,
    ) -> Result<KillTerminalCommandResponse, Error> {
        Ok(KillTerminalCommandResponse::new())
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> Result<TerminalOutputResponse, Error> {
        Ok(TerminalOutputResponse::new("output data", false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> Result<ReleaseTerminalResponse, Error> {
        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> Result<WaitForTerminalExitResponse, Error> {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        Ok(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(0u32),
        ))
    }
}

#[tokio::test]
async fn terminal_wait_for_exit_round_trip() {
    let client = RpcMockClient;
    let request = WaitForTerminalExitRequest::new("session-001", "term-001");
    let payload = serde_json::to_vec(&request).unwrap();

    let result = terminal_wait_for_exit::handle(&payload, &client, std::time::Duration::from_secs(30)).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn terminal_wait_for_exit_times_out_when_operation_expires() {
    let client = SlowTerminalClient;
    let request = WaitForTerminalExitRequest::new("session-001", "term-001");
    let payload = serde_json::to_vec(&request).unwrap();

    let result = terminal_wait_for_exit::handle(
        &payload,
        &client,
        std::time::Duration::from_millis(50),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Timed out waiting for terminal exit"));
}

#[tokio::test]
async fn request_permission_round_trip() {
    use agent_client_protocol::{
        PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
    };
    let client = RpcMockClient;
    let tool_call = ToolCallUpdate::new("tool-call-001", ToolCallUpdateFields::new());
    let options = vec![PermissionOption::new(
        "allow-once",
        "Allow once",
        PermissionOptionKind::AllowOnce,
    )];
    let request = RequestPermissionRequest::new("session-001", tool_call, options);
    let payload = serde_json::to_vec(&request).unwrap();

    let result = request_permission::handle(&payload, &client).await;
    assert!(result.is_ok());

    let response: RequestPermissionResponse = serde_json::from_slice(&result.unwrap()).unwrap();
    assert_eq!(response.outcome, RequestPermissionOutcome::Cancelled);
}

#[tokio::test]
async fn client_handler_invalid_payload_returns_error() {
    let client = RpcMockClient;
    let result = fs_read_text_file::handle(b"not json", &client, 1_048_576).await;
    assert!(result.is_err());
}
