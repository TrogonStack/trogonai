use super::*;
use crate::session_id::AcpSessionId;
use agent_client_protocol::{
    ContentBlock, ContentChunk, CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest,
    KillTerminalResponse, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    Request, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate, TerminalExitStatus, TerminalOutputRequest, TerminalOutputResponse,
    ToolCallUpdate, ToolCallUpdateFields, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    WriteTextFileRequest, WriteTextFileResponse,
};
use async_trait::async_trait;
use std::cell::RefCell;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
use trogon_std::time::SystemClock;
use async_nats::header::HeaderMap;
use jsonrpc_nats::RequestId;

pub(super) struct MockClient {
    notifications: RefCell<Vec<String>>,
    kill_terminal_calls: RefCell<usize>,
    terminal_output_calls: RefCell<usize>,
    terminal_release_calls: RefCell<usize>,
    wait_for_terminal_exit_calls: RefCell<usize>,
}

impl MockClient {
    pub(super) fn new() -> Self {
        Self {
            notifications: RefCell::new(Vec::new()),
            kill_terminal_calls: RefCell::new(0),
            terminal_output_calls: RefCell::new(0),
            terminal_release_calls: RefCell::new(0),
            wait_for_terminal_exit_calls: RefCell::new(0),
        }
    }

    pub(super) fn kill_terminal_call_count(&self) -> usize {
        *self.kill_terminal_calls.borrow()
    }

    pub(super) fn terminal_output_call_count(&self) -> usize {
        *self.terminal_output_calls.borrow()
    }

    pub(super) fn terminal_release_call_count(&self) -> usize {
        *self.terminal_release_calls.borrow()
    }

    pub(super) fn wait_for_terminal_exit_call_count(&self) -> usize {
        *self.wait_for_terminal_exit_calls.borrow()
    }
}

#[async_trait(?Send)]
impl Client for MockClient {
    async fn session_notification(
        &self,
        n: agent_client_protocol::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        self.notifications.borrow_mut().push(format!("{:?}", n));
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
        *self.kill_terminal_calls.borrow_mut() += 1;
        Ok(KillTerminalResponse::new())
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        *self.terminal_output_calls.borrow_mut() += 1;
        Ok(TerminalOutputResponse::new("mock output".to_string(), false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        *self.terminal_release_calls.borrow_mut() += 1;
        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        *self.wait_for_terminal_exit_calls.borrow_mut() += 1;
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

fn make_bridge(nats: MockNatsClient) -> Rc<Bridge<MockNatsClient, SystemClock, crate::agent::test_support::MockJs>> {
    Rc::new(Bridge::new(
        nats,
        crate::agent::test_support::MockJs::new(),
        SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        crate::config::Config::for_test("acp"),
        tokio::sync::mpsc::channel(1).0,
    ))
}

fn make_bridge_advanced(
    nats: AdvancedMockNatsClient,
) -> Rc<Bridge<AdvancedMockNatsClient, SystemClock, crate::agent::test_support::MockJs>> {
    Rc::new(Bridge::new(
        nats,
        crate::agent::test_support::MockJs::new(),
        SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        crate::config::Config::for_test("acp"),
        tokio::sync::mpsc::channel(1).0,
    ))
}

fn make_bridge_with_operation_timeout(
    nats: MockNatsClient,
    operation_timeout: std::time::Duration,
) -> Rc<Bridge<MockNatsClient, SystemClock, crate::agent::test_support::MockJs>> {
    Rc::new(Bridge::new(
        nats,
        crate::agent::test_support::MockJs::new(),
        SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        crate::config::Config::for_test("acp").with_operation_timeout(operation_timeout),
        tokio::sync::mpsc::channel(1).0,
    ))
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
    let client = Rc::new(MockClient::new());

    run(nats, client, bridge).await;
}

#[tokio::test]
async fn run_processes_messages_then_exits_when_stream_ends() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let nats = MockNatsClient::new();
            let bridge = make_bridge(nats.clone());
            let client = Rc::new(MockClient::new());

            let notification = SessionNotification::new(
                "sess1",
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
            );
            let (wire_headers, payload_bytes) = crate::client::test_support::encode_wire_notification("session/update", &notification);
            let msg = make_msg("acp.session.sess1.client.session.update", Some(wire_headers), &payload_bytes, None);

            let tx = nats.inject_messages();
            tx.unbounded_send(msg).unwrap();
            drop(tx);

            run(nats, client.clone(), bridge).await;

            tokio::task::yield_now().await;
            assert_eq!(client.notifications.borrow().len(), 1);
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
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_notification("session/update", &notification);
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
    dispatch_client_method("acp.session.sess-1.client.session.update", parsed, &headers, payload, None, &ctx).await;

    assert_eq!(client.notifications.borrow().len(), 1);
}

#[tokio::test]
async fn dispatch_client_method_dispatches_fs_read_text_file() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess-1");
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
        "acp.session.sess-1.client.fs.read_text_file",
        parsed,
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
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new("sess-1", "/tmp/foo"));
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
        "acp.session.sess-1.client.fs.read_text_file",
        parsed,
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
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new("sess-1", "/tmp/foo"));
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
        "acp.session.sess-1.client.fs.read_text_file",
        parsed,
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
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess-1");
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
        "acp.session.sess-1.client.fs.read_text_file",
        parsed,
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

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess-1");
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
        "acp.session.sess-1.client.fs.read_text_file",
        parsed,
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

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess-1");
    let payload = bytes::Bytes::from(payload_bytes);

    let parsed = crate::nats::ParsedClientSubject {
        session_id,
        method: ClientMethod::FsReadTextFile,
    };

    let bridge = make_bridge_advanced(nats.clone());
    let ctx = DispatchContext {
        nats: &nats,
        client: &client,
        bridge: &bridge,
    };
    dispatch_client_method(
        "acp.session.sess-1.client.fs.read_text_file",
        parsed,
        payload,
        Some("_INBOX.reply".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}


#[derive(Debug)]
struct RpcMockClient;

#[async_trait(?Send)]
impl Client for RpcMockClient {
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
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_notification("session/update", &notification);
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
    dispatch_client_method("acp.session.sess-1.client.session.update", parsed, &headers, payload, None, &ctx).await;
}

#[tokio::test]
async fn dispatch_client_method_dispatches_fs_read_text_file_with_rpc_mock_client() {
    let nats = MockNatsClient::new();
    let client = RpcMockClient;
    let session_id = AcpSessionId::new("sess-1").unwrap();

    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess-1");
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
        "acp.session.sess-1.client.fs.read_text_file",
        parsed,
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
        agent_client_protocol::ToolCallUpdate::new("call-1", agent_client_protocol::ToolCallUpdateFields::new()),
        vec![],
    );
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("session/request_permission", RequestId::Number(1), &request);
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
        "acp.session.sess-1.client.session.request_permission",
        parsed,
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
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("fs/write_text_file", RequestId::Number(1), &WriteTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess-1");
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
        "acp.session.sess-1.client.fs.write_text_file",
        parsed,
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
        agent_client_protocol::ToolCallUpdate::new("call-1", agent_client_protocol::ToolCallUpdateFields::new()),
        vec![],
    );
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("session/request_permission", RequestId::Number(1), &request);
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
        "acp.session.sess-1.client.session.request_permission",
        parsed,
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
        agent_client_protocol::ToolCallUpdate::new("call-1", agent_client_protocol::ToolCallUpdateFields::new()),
        vec![],
    );
    let (headers, payload_bytes) = crate::client::test_support::encode_wire_request("session/request_permission", RequestId::Number(1), &request);
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
        "acp.session.sess-1.client.session.request_permission",
        parsed,
        payload,
        Some("_INBOX.err".to_string()),
        &ctx,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}


#[tokio::test]
async fn process_message_invalid_subject_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Rc::new(MockClient::new());
    let in_flight = Rc::new(Cell::new(0usize));

    let msg = make_msg("acp.sess.unknown.method", None, b"{}", None);
    process_message(msg, &nats, client, bridge, &in_flight, 256).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn process_message_invalid_subject_with_reply_is_ignored() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Rc::new(MockClient::new());
    let in_flight = Rc::new(Cell::new(0usize));

    let msg = make_msg("acp.sess.unknown.method", None, b"{}", Some("_INBOX.reply"));
    process_message(msg, &nats, client, bridge, &in_flight, 256).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn process_message_backpressure_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Rc::new(MockClient::new());
    let in_flight = Rc::new(Cell::new(1usize));

    let msg = make_msg("acp.session.sess1.client.session.update", b"{}", None);
    process_message(msg, &nats, client, bridge, &in_flight, 1).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn process_message_backpressure_with_reply_publishes_error() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Rc::new(MockClient::new());
    let in_flight = Rc::new(Cell::new(1usize));

    let (headers, payload) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess1");
    let msg = make_msg(
        "acp.session.sess1.client.fs.read_text_file",
        &payload,
        Some("_INBOX.reply"),
    );
    process_message(msg, &nats, client, bridge, &in_flight, 1).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn process_message_backpressure_with_reply_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let bridge = make_bridge_advanced(nats.clone());
    let client = Rc::new(MockClient::new());
    let in_flight = Rc::new(Cell::new(1usize));

    let (headers, payload) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess1");
    let msg = make_msg(
        "acp.session.sess1.client.fs.read_text_file",
        &payload,
        Some("_INBOX.reply"),
    );
    process_message(msg, &nats, client, bridge, &in_flight, 1).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn process_message_backpressure_with_reply_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let bridge = make_bridge_advanced(nats.clone());
    let client = Rc::new(MockClient::new());
    let in_flight = Rc::new(Cell::new(1usize));

    let (headers, payload) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess1");
    let msg = make_msg(
        "acp.session.sess1.client.fs.read_text_file",
        &payload,
        Some("_INBOX.reply"),
    );
    process_message(msg, &nats, client, bridge, &in_flight, 1).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn process_message_backpressure_first_serialize_fails_uses_fallback() {
    let nats = MockNatsClient::new();
    let bridge = make_bridge(nats.clone());
    let client = Rc::new(MockClient::new());
    let in_flight = Rc::new(Cell::new(1usize));

    let (headers, payload) = crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), &ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess1");
    let msg = make_msg(
        "acp.session.sess1.client.fs.read_text_file",
        &payload,
        Some("_INBOX.reply"),
    );
    process_message(msg, &nats, client, bridge, &in_flight, 1).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}


#[tokio::test]
async fn process_message_valid_dispatch_spawns_task() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let nats = MockNatsClient::new();
            let bridge = make_bridge(nats.clone());
            let client = Rc::new(MockClient::new());
            let in_flight = Rc::new(Cell::new(0usize));

            let notification = SessionNotification::new(
                "sess1",
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
            );
            let (wire_headers, payload_bytes) = crate::client::test_support::encode_wire_notification("session/update", &notification);
            let msg = make_msg("acp.session.sess1.client.session.update", Some(wire_headers), &payload_bytes, None);
            process_message(msg, &nats, client, bridge, &in_flight, 256).await;

            // Yield to allow the spawned local task to run.
            tokio::task::yield_now().await;

            assert_eq!(client.notifications.borrow().len(), 1);
        })
        .await;
}
