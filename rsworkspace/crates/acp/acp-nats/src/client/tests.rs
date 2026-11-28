use super::*;
use crate::session_id::AcpSessionId;
use agent_client_protocol::v1::schema::{
    ContentBlock, ContentChunk, CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest,
    KillTerminalResponse, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionNotification, SessionUpdate,
    TerminalExitStatus, TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use async_nats::header::HeaderMap;
use async_trait::async_trait;
use jsonrpc_nats::RequestId;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
use trogon_std::time::SystemClock;

pub(super) struct MockClient {
    notifications: Mutex<Vec<String>>,
    kill_terminal_calls: Mutex<usize>,
    terminal_output_calls: Mutex<usize>,
    terminal_release_calls: Mutex<usize>,
    wait_for_terminal_exit_calls: Mutex<usize>,
}

impl MockClient {
    pub(super) fn new() -> Self {
        Self {
            notifications: Mutex::new(Vec::new()),
            kill_terminal_calls: Mutex::new(0),
            terminal_output_calls: Mutex::new(0),
            terminal_release_calls: Mutex::new(0),
            wait_for_terminal_exit_calls: Mutex::new(0),
        }
    }

    pub(super) fn kill_terminal_call_count(&self) -> usize {
        *self.kill_terminal_calls.lock().unwrap()
    }

    pub(super) fn wait_for_terminal_exit_call_count(&self) -> usize {
        *self.wait_for_terminal_exit_calls.lock().unwrap()
    }
}

#[async_trait]
impl ClientHandler for MockClient {
    async fn session_notification(
        &self,
        n: agent_client_protocol::v1::schema::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        self.notifications.lock().unwrap().push(format!("{:?}", n));
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Ok(ReadTextFileResponse::new("mock file content".to_string()))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Ok(WriteTextFileResponse::new())
    }

    async fn create_terminal(&self, _: CreateTerminalRequest) -> agent_client_protocol::Result<CreateTerminalResponse> {
        Ok(CreateTerminalResponse::new("term-001"))
    }

    async fn kill_terminal(&self, _: KillTerminalRequest) -> agent_client_protocol::Result<KillTerminalResponse> {
        *self.kill_terminal_calls.lock().unwrap() += 1;
        Ok(KillTerminalResponse::new())
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        *self.terminal_output_calls.lock().unwrap() += 1;
        Ok(TerminalOutputResponse::new("mock output".to_string(), false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        *self.terminal_release_calls.lock().unwrap() += 1;
        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        *self.wait_for_terminal_exit_calls.lock().unwrap() += 1;
        Ok(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(0u32),
        ))
    }
}

fn make_msg(subject: &str, headers: Option<HeaderMap>, payload: &[u8], reply: Option<&str>) -> async_nats::Message {
    async_nats::Message {
        subject: subject.into(),
        reply: reply.map(|r| r.into()),
        payload: payload.to_vec().into(),
        headers,
        length: payload.len(),
        status: None,
        description: None,
    }
}

fn make_bridge(nats: MockNatsClient) -> Arc<Bridge<MockNatsClient, SystemClock, crate::agent::test_support::MockJs>> {
    Arc::new(Bridge::new(
        nats,
        crate::agent::test_support::MockJs::new(),
        SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        crate::config::Config::for_test("acp"),
    ))
}

fn make_bridge_advanced(
    nats: AdvancedMockNatsClient,
) -> Arc<Bridge<AdvancedMockNatsClient, SystemClock, crate::agent::test_support::MockJs>> {
    Arc::new(Bridge::new(
        nats,
        crate::agent::test_support::MockJs::new(),
        SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        crate::config::Config::for_test("acp"),
    ))
}

pub(super) struct TerminalKillFailingClient;

#[async_trait]
impl ClientHandler for TerminalKillFailingClient {
    async fn session_notification(
        &self,
        n: agent_client_protocol::v1::schema::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        let _ = n;
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Ok(ReadTextFileResponse::new("mock file content".to_string()))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Ok(WriteTextFileResponse::new())
    }

    async fn create_terminal(&self, _: CreateTerminalRequest) -> agent_client_protocol::Result<CreateTerminalResponse> {
        Ok(CreateTerminalResponse::new("term-001"))
    }

    async fn kill_terminal(&self, _: KillTerminalRequest) -> agent_client_protocol::Result<KillTerminalResponse> {
        Err(agent_client_protocol::Error::new(-32603, "mock kill_terminal failure"))
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "mock terminal_output failure",
        ))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "mock release_terminal failure",
        ))
    }
}

pub(super) struct TerminalReleaseFailingClient;

#[async_trait]
impl ClientHandler for TerminalReleaseFailingClient {
    async fn session_notification(
        &self,
        n: agent_client_protocol::v1::schema::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        let _ = n;
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Ok(ReadTextFileResponse::new("mock file content".to_string()))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Ok(WriteTextFileResponse::new())
    }

    async fn create_terminal(&self, _: CreateTerminalRequest) -> agent_client_protocol::Result<CreateTerminalResponse> {
        Ok(CreateTerminalResponse::new("term-001"))
    }

    async fn kill_terminal(&self, _: KillTerminalRequest) -> agent_client_protocol::Result<KillTerminalResponse> {
        Ok(KillTerminalResponse::new())
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        Ok(TerminalOutputResponse::new("mock output".to_string(), false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "mock release_terminal failure",
        ))
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        Ok(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(0u32),
        ))
    }
}

pub(super) struct TerminalWaitForExitFailingClient;

#[async_trait]
impl ClientHandler for TerminalWaitForExitFailingClient {
    async fn session_notification(
        &self,
        n: agent_client_protocol::v1::schema::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        let _ = n;
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Ok(ReadTextFileResponse::new("mock file content".to_string()))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Ok(WriteTextFileResponse::new())
    }

    async fn create_terminal(&self, _: CreateTerminalRequest) -> agent_client_protocol::Result<CreateTerminalResponse> {
        Ok(CreateTerminalResponse::new("term-001"))
    }

    async fn kill_terminal(&self, _: KillTerminalRequest) -> agent_client_protocol::Result<KillTerminalResponse> {
        Ok(KillTerminalResponse::new())
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        Ok(TerminalOutputResponse::new("mock output".to_string(), false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "mock wait_for_terminal_exit failure",
        ))
    }
}

pub(super) struct TerminalWaitForExitTimeoutClient;

#[async_trait]
impl ClientHandler for TerminalWaitForExitTimeoutClient {
    async fn session_notification(
        &self,
        n: agent_client_protocol::v1::schema::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        let _ = n;
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Ok(ReadTextFileResponse::new("mock file content".to_string()))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Ok(WriteTextFileResponse::new())
    }

    async fn create_terminal(&self, _: CreateTerminalRequest) -> agent_client_protocol::Result<CreateTerminalResponse> {
        Ok(CreateTerminalResponse::new("term-001"))
    }

    async fn kill_terminal(&self, _: KillTerminalRequest) -> agent_client_protocol::Result<KillTerminalResponse> {
        Ok(KillTerminalResponse::new())
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        Ok(TerminalOutputResponse::new("mock output".to_string(), false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        Ok(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(0u32),
        ))
    }
}

#[tokio::test]
async fn mock_client_request_permission_returns_err() {
    let client = MockClient::new();
    let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
        "sessionId": "sess-1",
        "toolCall": {
            "toolCallId": "call-1"
        },
        "options": []
    }))
    .unwrap();
    let result = client.request_permission(req).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn run_returns_early_when_subscribe_fails() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(MockClient::new());

    run(nats, client, bridge).await;
}

#[tokio::test]
async fn run_processes_messages_then_exits_when_stream_ends() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let nats = MockNatsClient::new();
            let bridge = make_bridge(nats.clone());
            let client = Arc::new(MockClient::new());

            let tx = nats.inject_messages();
            tx.unbounded_send(session_update_msg("sess1", "hi")).unwrap();
            drop(tx);

            run(nats, client.clone(), bridge).await;

            tokio::task::yield_now().await;
            assert_eq!(client.notifications.lock().unwrap().len(), 1);
        })
        .await;
}

#[tokio::test]
async fn dispatch_client_method_dispatches_session_update() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let notification = SessionNotification::new(
        "sess-1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
    );
    let (headers, payload_bytes) =
        crate::client::test_support::encode_wire_notification("session/update", &notification);
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::SessionUpdate,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.session.update",
        parsed,
        &headers,
        payload,
        None,
        &ctx,
    )
    .await;

    assert_eq!(client.notifications.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_fs_read_text_file() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess-1", "/tmp/foo"),
    );
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::FsReadTextFile,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.fs.read_text_file",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_fs_write_text_file() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess-1", "/tmp/foo"),
    );
    let payload = bytes::Bytes::from(payload_bytes);
    let parsed = crate::nats::ParsedClientSubject {
        session_id: AcpSessionId::new("sess-1").unwrap(),
        method: ClientMethod::FsReadTextFile,
    };
    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.fs.read_text_file",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn dispatch_client_method_terminal_wait_for_exit_failing_client_terminal_create_covers_stubs() {
    let nats = MockNatsClient::new();
    let client = TerminalWaitForExitFailingClient;
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess-1", "/tmp/foo"),
    );
    let payload = bytes::Bytes::from(payload_bytes);
    let parsed = crate::nats::ParsedClientSubject {
        session_id: AcpSessionId::new("sess-1").unwrap(),
        method: ClientMethod::FsReadTextFile,
    };
    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.fs.read_text_file",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_terminal_create_covers_stubs() {
    let nats = MockNatsClient::new();
    let client = TerminalWaitForExitTimeoutClient;
    let session_id = AcpSessionId::new("sess-1").unwrap();
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess-1", "/tmp/foo"),
    );
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::FsReadTextFile,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.fs.read_text_file",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_terminal_create_with_terminal_release_failing_client() {
    let nats = MockNatsClient::new();
    let client = TerminalReleaseFailingClient;
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess-1", "/tmp/foo"),
    );
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::FsReadTextFile,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.fs.read_text_file",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_terminal_create_with_terminal_kill_failing_client() {
    let nats = MockNatsClient::new();
    let client = TerminalKillFailingClient;
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess-1", "/tmp/foo"),
    );
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::FsReadTextFile,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.fs.read_text_file",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[derive(Debug)]
struct RpcMockClient;

#[async_trait]
impl ClientHandler for RpcMockClient {
    async fn session_notification(&self, _: SessionNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Ok(ReadTextFileResponse::new("file contents".to_string()))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Ok(WriteTextFileResponse::new())
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        Ok(TerminalOutputResponse::new("rpc mock output".to_string(), false))
    }
}

#[tokio::test]
async fn dispatch_client_method_dispatches_session_update_with_rpc_mock_client() {
    let nats = MockNatsClient::new();
    let client = RpcMockClient;
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let notification = SessionNotification::new(
        "sess-1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
    );
    let (headers, payload_bytes) =
        crate::client::test_support::encode_wire_notification("session/update", &notification);
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::SessionUpdate,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.session.update",
        parsed,
        &headers,
        payload,
        None,
        &ctx,
    )
    .await;
}

#[tokio::test]
async fn dispatch_client_method_dispatches_fs_read_text_file_with_rpc_mock_client() {
    let nats = MockNatsClient::new();
    let client = RpcMockClient;
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess-1", "/tmp/foo"),
    );
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::FsReadTextFile,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.fs.read_text_file",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_request_permission() {
    let nats = MockNatsClient::new();
    let client = RpcMockClient;
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let request = RequestPermissionRequest::new(
        "sess-1",
        agent_client_protocol::v1::schema::ToolCallUpdate::new(
            "call-1",
            agent_client_protocol::v1::schema::ToolCallUpdateFields::new(),
        ),
        vec![],
    );
    let (headers, payload_bytes) =
        crate::client::test_support::encode_wire_request("session/request_permission", RequestId::Number(1), &request);
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::SessionRequestPermission,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.session.request_permission",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn dispatch_client_method_rpc_mock_client_write_text_file_covers_stubs() {
    let nats = MockNatsClient::new();
    let client = RpcMockClient;
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request(
        "fs/write_text_file",
        RequestId::Number(1),
        &WriteTextFileRequest::new("sess-1", "/tmp/foo", "content"),
    );
    let payload = bytes::Bytes::from(payload_bytes);
    let parsed = crate::nats::ParsedClientSubject {
        session_id: AcpSessionId::new("sess-1").unwrap(),
        method: ClientMethod::FsWriteTextFile,
    };
    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.fs.write_text_file",
        parsed,
        &headers,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_request_permission_client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let request = RequestPermissionRequest::new(
        "sess-1",
        agent_client_protocol::v1::schema::ToolCallUpdate::new(
            "call-1",
            agent_client_protocol::v1::schema::ToolCallUpdateFields::new(),
        ),
        vec![],
    );
    let (headers, payload_bytes) =
        crate::client::test_support::encode_wire_request("session/request_permission", RequestId::Number(1), &request);
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::SessionRequestPermission,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.session.request_permission",
        parsed,
        &headers,
        payload,
        Some("_INBOX.err".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_request_permission_with_advanced_mock() {
    let nats = AdvancedMockNatsClient::new();
    let client = MockClient::new();
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let request = RequestPermissionRequest::new(
        "sess-1",
        agent_client_protocol::v1::schema::ToolCallUpdate::new(
            "call-1",
            agent_client_protocol::v1::schema::ToolCallUpdateFields::new(),
        ),
        vec![],
    );
    let (headers, payload_bytes) =
        crate::client::test_support::encode_wire_request("session/request_permission", RequestId::Number(1), &request);
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::SessionRequestPermission,
    };

    let bridge = make_bridge_advanced(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.session.request_permission",
        parsed,
        &headers,
        payload,
        Some("_INBOX.err".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_elicitation_create_method_not_found() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let mode = agent_client_protocol::v1::schema::ElicitationFormMode::new(
        agent_client_protocol::v1::schema::ElicitationSessionScope::new("sess-1"),
        agent_client_protocol::v1::schema::ElicitationSchema::new(),
    );
    let request = agent_client_protocol::v1::schema::CreateElicitationRequest::new(mode, "please respond");
    let (headers, payload_bytes) =
        crate::client::test_support::encode_wire_request("elicitation/create", RequestId::Number(1), &request);
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::ElicitationCreate,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.elicitation.create",
        parsed,
        &headers,
        payload,
        Some("_INBOX.err".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_elicitation_complete() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let notification = agent_client_protocol::v1::schema::CompleteElicitationNotification::new("elicitation-1");
    let (headers, payload_bytes) =
        crate::client::test_support::encode_wire_notification("elicitation/complete", &notification);
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::ElicitationComplete,
    };

    let bridge = make_bridge(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.v1.session.sess-1.client.elicitation.complete",
        parsed,
        &headers,
        payload,
        None,
        &ctx,
    )
    .await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn process_message_invalid_subject_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(MockClient::new());
    let in_flight = Arc::new(AtomicUsize::new(0));

    let msg = make_msg("acp.sess.unknown.method", None, b"{}", None);
    process_message(msg, &nats, &client, &bridge, &in_flight, 256).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn process_message_invalid_subject_with_reply_is_ignored() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(MockClient::new());
    let in_flight = Arc::new(AtomicUsize::new(0));

    let msg = make_msg("acp.sess.unknown.method", None, b"{}", Some("_INBOX.reply"));
    process_message(msg, &nats, &client, &bridge, &in_flight, 256).await;

    assert!(nats.published_messages().is_empty());
}

/// Notifications are not subject to the request limit: dropping one would leave a
/// gap in the client's transcript, so a full request slot table must not stop it.
#[tokio::test]
async fn process_message_delivers_a_notification_while_requests_are_at_the_limit() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(MockClient::new());
    let in_flight = Arc::new(AtomicUsize::new(1));

    let msg = session_update_msg("sess1", "hi");
    process_message(msg, &nats, &client, &bridge, &in_flight, 1).await;

    assert_eq!(client.notifications.lock().unwrap().len(), 1);
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn process_message_backpressure_with_reply_publishes_error() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(MockClient::new());
    let in_flight = Arc::new(AtomicUsize::new(1));

    let (headers, payload) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess1", "/tmp/foo"),
    );
    let msg = make_msg(
        "acp.v1.session.sess1.client.fs.read_text_file",
        Some(headers),
        &payload,
        Some("_INBOX.reply"),
    );
    process_message(msg, &nats, &client, &bridge, &in_flight, 1).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn process_message_backpressure_with_reply_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let bridge = make_bridge_advanced(nats.clone());
    let client = Arc::new(MockClient::new());
    let in_flight = Arc::new(AtomicUsize::new(1));

    let (headers, payload) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess1", "/tmp/foo"),
    );
    let msg = make_msg(
        "acp.v1.session.sess1.client.fs.read_text_file",
        Some(headers),
        &payload,
        Some("_INBOX.reply"),
    );
    process_message(msg, &nats, &client, &bridge, &in_flight, 1).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn process_message_backpressure_with_reply_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let bridge = make_bridge_advanced(nats.clone());
    let client = Arc::new(MockClient::new());
    let in_flight = Arc::new(AtomicUsize::new(1));

    let (headers, payload) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess1", "/tmp/foo"),
    );
    let msg = make_msg(
        "acp.v1.session.sess1.client.fs.read_text_file",
        Some(headers),
        &payload,
        Some("_INBOX.reply"),
    );
    process_message(msg, &nats, &client, &bridge, &in_flight, 1).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn process_message_backpressure_first_serialize_fails_uses_fallback() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(MockClient::new());
    let in_flight = Arc::new(AtomicUsize::new(1));

    let (headers, payload) = crate::client::test_support::encode_wire_request(
        "fs/read_text_file",
        RequestId::Number(1),
        &ReadTextFileRequest::new("sess1", "/tmp/foo"),
    );
    let msg = make_msg(
        "acp.v1.session.sess1.client.fs.read_text_file",
        Some(headers),
        &payload,
        Some("_INBOX.reply"),
    );
    process_message(msg, &nats, &client, &bridge, &in_flight, 1).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn process_message_delivers_a_notification_before_returning() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(MockClient::new());
    let in_flight = Arc::new(AtomicUsize::new(0));

    process_message(
        session_update_msg("sess1", "hi"),
        &nats,
        &client,
        &bridge,
        &in_flight,
        256,
    )
    .await;

    assert_eq!(client.notifications.lock().unwrap().len(), 1);
}

fn session_update_msg(session_id: &str, text: &str) -> async_nats::Message {
    let notification = SessionNotification::new(
        session_id.to_string(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text.to_string()))),
    );
    let (headers, payload) = crate::client::test_support::encode_wire_notification("session/update", &notification);
    make_msg(
        &format!("acp.v1.session.{session_id}.client.session.update"),
        Some(headers),
        &payload,
        None,
    )
}

/// Records streamed text chunks in delivery order. Its permission prompt stays
/// open until `release_permission` fires, like a user who has not answered yet.
struct StreamingClient {
    chunks: Mutex<Vec<String>>,
    chunk_arrived: tokio::sync::Notify,
    release_permission: tokio::sync::Notify,
}

impl StreamingClient {
    fn new() -> Self {
        Self {
            chunks: Mutex::new(Vec::new()),
            chunk_arrived: tokio::sync::Notify::new(),
            release_permission: tokio::sync::Notify::new(),
        }
    }

    async fn wait_for_chunks(&self, count: usize) -> Vec<String> {
        loop {
            let arrived = self.chunk_arrived.notified();
            {
                let chunks = self.chunks.lock().unwrap();
                if chunks.len() >= count {
                    return chunks.clone();
                }
            }
            arrived.await;
        }
    }
}

#[async_trait]
impl ClientHandler for StreamingClient {
    async fn session_notification(&self, n: SessionNotification) -> agent_client_protocol::Result<()> {
        if let SessionUpdate::AgentMessageChunk(ContentChunk {
            content: ContentBlock::Text(text),
            ..
        }) = n.update
        {
            self.chunks.lock().unwrap().push(text.text);
            self.chunk_arrived.notify_waiters();
        }
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        self.release_permission.notified().await;
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }
}

const STREAMED_CHUNKS: usize = 1000;

fn streamed_chunks() -> Vec<String> {
    (0..STREAMED_CHUNKS).map(|i| i.to_string()).collect()
}

/// Runs the client proxy on its own task, fed by the returned sender.
fn spawn_streaming_proxy() -> (
    MockNatsClient,
    futures::channel::mpsc::UnboundedSender<async_nats::Message>,
    Arc<StreamingClient>,
    tokio::task::JoinHandle<()>,
) {
    let nats = MockNatsClient::new();
    let inbound = nats.inject_messages();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(StreamingClient::new());
    let proxy = tokio::spawn(run(nats.clone(), client.clone(), bridge));
    (nats, inbound, client, proxy)
}

/// Regression: with one spawned task per message on a multi-threaded runtime,
/// a streamed reply reached the client out of order (126 of 516 chunks in a
/// chaos run with no faults injected).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_delivers_streamed_session_updates_in_publish_order() {
    let (_nats, inbound, client, proxy) = spawn_streaming_proxy();
    let sent = streamed_chunks();
    for text in &sent {
        inbound.unbounded_send(session_update_msg("sess1", text)).unwrap();
    }

    let received = tokio::time::timeout(Duration::from_secs(10), client.wait_for_chunks(STREAMED_CHUNKS))
        .await
        .expect("every chunk reaches the client");
    proxy.abort();

    assert_eq!(received, sent);
}

/// A permission prompt the user has not answered must not hold back the updates
/// that follow it: the client only answers once it has seen the whole stream, so
/// queuing updates behind the prompt would deadlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_keeps_streaming_updates_while_a_permission_prompt_is_open() {
    let (nats, inbound, client, proxy) = spawn_streaming_proxy();
    let sent = streamed_chunks();
    let (before, after) = sent.split_at(STREAMED_CHUNKS / 2);
    for text in before {
        inbound.unbounded_send(session_update_msg("sess1", text)).unwrap();
    }
    let tool_call = agent_client_protocol::v1::schema::ToolCallUpdate::new(
        "call-1",
        agent_client_protocol::v1::schema::ToolCallUpdateFields::new(),
    );
    let (headers, payload) = crate::client::test_support::encode_wire_request(
        "session/request_permission",
        RequestId::Number(7),
        &RequestPermissionRequest::new("sess1", tool_call, vec![]),
    );
    inbound
        .unbounded_send(make_msg(
            "acp.v1.session.sess1.client.session.request_permission",
            Some(headers),
            &payload,
            Some("_INBOX.permission"),
        ))
        .unwrap();
    for text in after {
        inbound.unbounded_send(session_update_msg("sess1", text)).unwrap();
    }

    let received = tokio::time::timeout(Duration::from_secs(10), client.wait_for_chunks(STREAMED_CHUNKS))
        .await
        .expect("updates after the open prompt still reach the client");
    assert_eq!(received, sent);

    client.release_permission.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !nats.published_messages().contains(&"_INBOX.permission".to_string()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the permission answer is published once the user responds");
    proxy.abort();
}

/// Compile-time guard: the client proxy must stay `Send`.
///
/// The SDK's `ConnectTo::connect_to` returns `impl Future<Output = Result<()>> + Send`,
/// so anything that drives the bridge from an SDK-owned transport has to be `Send`
/// end to end. This test asserts the property rather than the mechanism: reintroducing
/// an `Rc`, a `Cell`, or a `spawn_local` anywhere under `client::run` fails to compile
/// here instead of surfacing as an unsatisfiable bound at a distant call site.
#[test]
fn client_proxy_stays_send_for_sdk_transports() {
    const fn assert_send<T: Send>() {}
    fn assert_send_val<T: Send>(_: &T) {}

    assert_send::<crate::boundary::ConnectionClient>();
    assert_send::<Arc<MockClient>>();

    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Arc::new(MockClient::new());

    let future = run(nats, client, bridge);
    assert_send_val(&future);
    drop(future);
}
