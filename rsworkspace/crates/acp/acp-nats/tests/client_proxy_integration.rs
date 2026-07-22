#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for acp-nats client proxy (`client::run()`) with a real NATS server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p acp-nats --test client_proxy_integration

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::client;
use acp_nats::{AcpPrefix, Bridge, ClientHandler, Config, NatsAuth, NatsConfig};
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest,
    KillTerminalResponse, PromptResponse, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    ReleaseTerminalResponse, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionId, SessionNotification, SessionUpdate, StopReason, TerminalExitStatus, TerminalOutputRequest,
    TerminalOutputResponse, ToolCallUpdate, ToolCallUpdateFields, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use async_trait::async_trait;
use bytes::Bytes;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_std::time::SystemClock;

// ── Mock client ───────────────────────────────────────────────────────────────

struct MockClient {
    calls: Mutex<Vec<String>>,
    read_file_content: String,
    terminal_id: String,
}

impl MockClient {
    fn new() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            read_file_content: "file content".to_string(),
            terminal_id: "term-001".to_string(),
        }
    }

    fn with_read_content(mut self, content: &str) -> Self {
        self.read_file_content = content.to_string();
        self
    }

    #[allow(dead_code)]
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ClientHandler for MockClient {
    async fn session_notification(&self, notification: SessionNotification) -> agent_client_protocol::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("session_notification:{:?}", notification));
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        self.calls.lock().unwrap().push("request_permission".to_string());
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        self.calls.lock().unwrap().push("read_text_file".to_string());
        Ok(ReadTextFileResponse::new(self.read_file_content.clone()))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        self.calls.lock().unwrap().push("write_text_file".to_string());
        Ok(WriteTextFileResponse::new())
    }

    async fn create_terminal(&self, _: CreateTerminalRequest) -> agent_client_protocol::Result<CreateTerminalResponse> {
        self.calls.lock().unwrap().push("create_terminal".to_string());
        Ok(CreateTerminalResponse::new(self.terminal_id.clone()))
    }

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        self.calls.lock().unwrap().push("terminal_output".to_string());
        Ok(TerminalOutputResponse::new("some output", false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        self.calls.lock().unwrap().push("release_terminal".to_string());
        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        self.calls
            .lock()
            .unwrap()
            .push("wait_for_terminal_exit".to_string());
        Ok(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(0u32),
        ))
    }

    async fn kill_terminal(&self, _: KillTerminalRequest) -> agent_client_protocol::Result<KillTerminalResponse> {
        self.calls.lock().unwrap().push("kill_terminal".to_string());
        Ok(KillTerminalResponse::new())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS")
}

fn make_bridge(
    nats: async_nats::Client,
    prefix: &str,
) -> Bridge<async_nats::Client, SystemClock, trogon_nats::jetstream::NatsJetStreamClient> {
    let config = Config::new(
        AcpPrefix::new(prefix).unwrap(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500));
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(async_nats::jetstream::new(nats.clone()));
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Bridge::new(
        nats,
        js_client,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-client-proxy-test"),
        config,
        tx,
    )
}

/// Encodes `params` as an ADR-0011 JSON-RPC-over-NATS request (method + id in
/// headers, params in body) and sends it request-reply over the raw NATS client.
async fn wire_request<P: serde::Serialize>(
    nats: &async_nats::Client,
    subject: &str,
    method: &str,
    params: &P,
) -> async_nats::Message {
    let encoded = jsonrpc_nats::encode(&jsonrpc_nats::Message::Request {
        id: jsonrpc_nats::RequestId::Number(1),
        method: method.to_string(),
        params: serde_json::to_value(params).unwrap(),
    })
    .unwrap();
    nats.request_with_headers(subject.to_string(), encoded.headers, encoded.body)
        .await
        .expect("request must succeed")
}

/// Decodes a JSON-RPC-over-NATS success reply and returns its `result` value,
/// panicking with the decoded message when the reply is not a success.
fn wire_success_result(reply: &async_nats::Message) -> serde_json::Value {
    let decoded = jsonrpc_nats::decode(
        jsonrpc_nats::Direction::Response,
        None,
        reply.headers.as_ref().expect("reply must carry headers"),
        &reply.payload,
    )
    .expect("reply must decode");
    match decoded {
        jsonrpc_nats::Message::Success { result, .. } => result,
        other => panic!("expected success reply, got: {other:?}"),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fs_read_text_file_through_proxy_returns_file_content() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new().with_read_content("file content");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let reply = wire_request(
                &nats1,
                "acp.session.sess-1.client.fs.read_text_file",
                "fs/read_text_file",
                &ReadTextFileRequest::new(SessionId::from("sess-1"), "/tmp/test.txt"),
            )
            .await;

            let result = wire_success_result(&reply);
            assert_eq!(result["content"].as_str().unwrap(), "file content");
        })
        .await;
}

#[tokio::test]
async fn fs_write_text_file_through_proxy_returns_success() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let reply = wire_request(
                &nats1,
                "acp.session.sess-1.client.fs.write_text_file",
                "fs/write_text_file",
                &WriteTextFileRequest::new(
                    SessionId::from("sess-1"),
                    "/tmp/test.txt",
                    "hello",
                ),
            )
            .await;

            let result = wire_success_result(&reply);
            assert!(result.is_object(), "expected object result, got: {result}");
        })
        .await;
}

#[tokio::test]
async fn request_permission_through_proxy_returns_outcome() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
            let reply = wire_request(
                &nats1,
                "acp.session.sess-1.client.session.request_permission",
                "session/request_permission",
                &RequestPermissionRequest::new("sess-1", tool_call, vec![]),
            )
            .await;

            let response: RequestPermissionResponse =
                serde_json::from_value(wire_success_result(&reply)).expect("response must deserialize");
            assert_eq!(
                response.outcome,
                RequestPermissionOutcome::Cancelled,
                "MockClient returns Cancelled"
            );
        })
        .await;
}

#[tokio::test]
async fn session_update_through_proxy_calls_client() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");

    // We need a way to verify the call happened. Use an Arc<AtomicBool> so the
    // check survives the LocalSet boundary (the mock uses RefCell inside, but we
    // observe the side-effect via a shared atomic flag set from session_notification).
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();

    struct TrackingClient {
        called: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl ClientHandler for TrackingClient {
        async fn session_notification(&self, _: SessionNotification) -> agent_client_protocol::Result<()> {
            self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        async fn request_permission(
            &self,
            _: RequestPermissionRequest,
        ) -> agent_client_protocol::Result<RequestPermissionResponse> {
            Err(agent_client_protocol::Error::new(-32603, "not implemented"))
        }
    }

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(TrackingClient { called: called_clone });
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let notification = SessionNotification::new(
                "sess-1",
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
            );
            let payload = serde_json::to_vec(&notification).unwrap();
            nats1
                .publish("acp.session.sess-1.client.session.update", Bytes::from(payload))
                .await
                .unwrap();

            // Give the proxy time to process the notification
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;

    assert!(
        called.load(std::sync::atomic::Ordering::SeqCst),
        "expected session_notification to be called"
    );
}

#[tokio::test]
async fn terminal_create_through_proxy_returns_terminal_id() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let reply = wire_request(
                &nats1,
                "acp.session.sess-1.client.terminal.create",
                "terminal/create",
                &CreateTerminalRequest::new("sess-1", "echo hello"),
            )
            .await;

            let result = wire_success_result(&reply);
            assert!(result.is_object(), "expected object result, got: {result}");
            assert!(
                result.get("terminalId").is_some(),
                "expected terminalId field, got: {result}"
            );
        })
        .await;
}

#[tokio::test]
async fn terminal_output_through_proxy_returns_success() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let reply = wire_request(
                &nats1,
                "acp.session.sess-1.client.terminal.output",
                "terminal/output",
                &TerminalOutputRequest::new(SessionId::from("sess-1"), "term-001"),
            )
            .await;

            let result = wire_success_result(&reply);
            assert!(result.is_object(), "expected object result, got: {result}");
        })
        .await;
}

#[tokio::test]
async fn terminal_release_through_proxy_returns_success() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let reply = wire_request(
                &nats1,
                "acp.session.sess-1.client.terminal.release",
                "terminal/release",
                &ReleaseTerminalRequest::new(SessionId::from("sess-1"), "term-001"),
            )
            .await;

            let result = wire_success_result(&reply);
            assert!(result.is_object(), "expected object result, got: {result}");
        })
        .await;
}

#[tokio::test]
async fn terminal_wait_for_exit_through_proxy_returns_exit_code() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let reply = wire_request(
                &nats1,
                "acp.session.sess-1.client.terminal.wait_for_exit",
                "terminal/wait_for_exit",
                &WaitForTerminalExitRequest::new(SessionId::from("sess-1"), "term-001"),
            )
            .await;

            let result = wire_success_result(&reply);
            assert!(result.is_object(), "expected object result, got: {result}");
        })
        .await;
}

#[tokio::test]
async fn ext_session_prompt_response_through_proxy_does_not_panic() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            // Fire-and-forget: publish a valid PromptResponse (no reply subject expected)
            let response = PromptResponse::new(StopReason::EndTurn);
            let payload = serde_json::to_vec(&response).unwrap();
            nats1
                .publish(
                    "acp.session.sess-1.client.ext.session.prompt_response",
                    Bytes::from(payload),
                )
                .await
                .expect("publish must not fail");

            // Give the proxy time to process (should not crash)
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;
    // If we reach here without a panic the test passes
}

#[tokio::test]
async fn terminal_kill_through_proxy_returns_success() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats_client(port).await;

    let bridge = make_bridge(nats2.clone(), "acp");
    let mock_client = MockClient::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let client_rc = Rc::new(mock_client);
            let bridge_rc = Arc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let reply = wire_request(
                &nats1,
                "acp.session.sess-1.client.terminal.kill",
                "terminal/kill",
                &KillTerminalRequest::new(SessionId::from("sess-1"), "term-001".to_string()),
            )
            .await;

            let result = wire_success_result(&reply);
            assert!(result.is_object(), "expected object result, got: {result}");
        })
        .await;
}
