//! Integration tests for acp-nats client proxy (`client::run()`) with a real NATS server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p acp-nats --test client_proxy_integration

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use acp_nats::client;
use acp_nats::{AcpPrefix, Bridge, Config, NatsAuth, NatsConfig, StdJsonSerialize};
use agent_client_protocol::{
    Client, CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest,
    KillTerminalResponse, PromptResponse, ReadTextFileRequest, ReadTextFileResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, Request, RequestId, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification, SessionUpdate, StopReason, TerminalExitStatus,
    TerminalOutputRequest, TerminalOutputResponse, ToolCallUpdate, ToolCallUpdateFields,
    WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use async_trait::async_trait;
use bytes::Bytes;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_std::time::SystemClock;

// ── Mock client ───────────────────────────────────────────────────────────────

struct MockClient {
    calls: RefCell<Vec<String>>,
    read_file_content: String,
    terminal_id: String,
}

impl MockClient {
    fn new() -> Self {
        Self {
            calls: RefCell::new(vec![]),
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
        self.calls.borrow().clone()
    }
}

#[async_trait(?Send)]
impl Client for MockClient {
    async fn session_notification(
        &self,
        notification: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        self.calls
            .borrow_mut()
            .push(format!("session_notification:{:?}", notification));
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        self.calls
            .borrow_mut()
            .push("request_permission".to_string());
        Ok(RequestPermissionResponse::new(
            agent_client_protocol::RequestPermissionOutcome::Cancelled,
        ))
    }

    async fn read_text_file(
        &self,
        _: ReadTextFileRequest,
    ) -> agent_client_protocol::Result<ReadTextFileResponse> {
        self.calls.borrow_mut().push("read_text_file".to_string());
        Ok(ReadTextFileResponse::new(self.read_file_content.clone()))
    }

    async fn write_text_file(
        &self,
        _: WriteTextFileRequest,
    ) -> agent_client_protocol::Result<WriteTextFileResponse> {
        self.calls.borrow_mut().push("write_text_file".to_string());
        Ok(WriteTextFileResponse::new())
    }

    async fn create_terminal(
        &self,
        _: CreateTerminalRequest,
    ) -> agent_client_protocol::Result<CreateTerminalResponse> {
        self.calls.borrow_mut().push("create_terminal".to_string());
        Ok(CreateTerminalResponse::new(self.terminal_id.clone()))
    }

    async fn terminal_output(
        &self,
        _: TerminalOutputRequest,
    ) -> agent_client_protocol::Result<TerminalOutputResponse> {
        self.calls.borrow_mut().push("terminal_output".to_string());
        Ok(TerminalOutputResponse::new("some output", false))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        self.calls.borrow_mut().push("release_terminal".to_string());
        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        self.calls
            .borrow_mut()
            .push("wait_for_terminal_exit".to_string());
        Ok(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(0u32),
        ))
    }

    async fn kill_terminal(
        &self,
        _: KillTerminalRequest,
    ) -> agent_client_protocol::Result<KillTerminalResponse> {
        self.calls.borrow_mut().push("kill_terminal".to_string());
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

fn make_bridge(nats: async_nats::Client, prefix: &str) -> Bridge<async_nats::Client, SystemClock> {
    let config = Config::new(
        AcpPrefix::new(prefix).unwrap(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500));
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Bridge::new(
        nats,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-client-proxy-test"),
        config,
        tx,
    )
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let envelope = Request {
                id: RequestId::Number(1),
                method: std::sync::Arc::from("fs/read_text_file"),
                params: Some(ReadTextFileRequest::new(
                    agent_client_protocol::SessionId::from("sess-1"),
                    "/tmp/test.txt",
                )),
            };
            let payload = serde_json::to_vec(&envelope).unwrap();
            let reply = nats1
                .request("acp.sess-1.client.fs.read_text_file", Bytes::from(payload))
                .await
                .expect("request must succeed");

            let response: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
            assert!(
                response["result"].is_object(),
                "expected result in reply, got: {}",
                response
            );
            assert_eq!(
                response["result"]["content"].as_str().unwrap(),
                "file content"
            );
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let envelope = Request {
                id: RequestId::Number(2),
                method: std::sync::Arc::from("fs/write_text_file"),
                params: Some(WriteTextFileRequest::new(
                    agent_client_protocol::SessionId::from("sess-1"),
                    "/tmp/test.txt",
                    "hello",
                )),
            };
            let payload = serde_json::to_vec(&envelope).unwrap();
            let reply = nats1
                .request("acp.sess-1.client.fs.write_text_file", Bytes::from(payload))
                .await
                .expect("request must succeed");

            let response: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
            assert!(
                response.get("error").is_none(),
                "expected no error in reply, got: {}",
                response
            );
            assert!(
                response["result"].is_object(),
                "expected result in reply, got: {}",
                response
            );
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
            let envelope = Request {
                id: RequestId::Number(3),
                method: std::sync::Arc::from("session/request_permission"),
                params: Some(RequestPermissionRequest::new("sess-1", tool_call, vec![])),
            };
            let payload = serde_json::to_vec(&envelope).unwrap();
            let reply = nats1
                .request(
                    "acp.sess-1.client.session.request_permission",
                    Bytes::from(payload),
                )
                .await
                .expect("request must succeed");

            let response: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
            assert!(
                response["result"].is_object(),
                "expected result in reply, got: {}",
                response
            );
            assert!(
                response["result"].get("outcome").is_some(),
                "expected outcome field, got: {}",
                response["result"]
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

    #[async_trait(?Send)]
    impl Client for TrackingClient {
        async fn session_notification(
            &self,
            _: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
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
            let client_rc = Rc::new(TrackingClient {
                called: called_clone,
            });
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let notification = SessionNotification::new(
                "sess-1",
                SessionUpdate::AgentMessageChunk(agent_client_protocol::ContentChunk::new(
                    agent_client_protocol::ContentBlock::from("hello"),
                )),
            );
            let payload = serde_json::to_vec(&notification).unwrap();
            nats1
                .publish("acp.sess-1.client.session.update", Bytes::from(payload))
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let envelope = Request {
                id: RequestId::Number(5),
                method: std::sync::Arc::from("terminal/create"),
                params: Some(CreateTerminalRequest::new("sess-1", "echo hello")),
            };
            let payload = serde_json::to_vec(&envelope).unwrap();
            let reply = nats1
                .request("acp.sess-1.client.terminal.create", Bytes::from(payload))
                .await
                .expect("request must succeed");

            let response: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
            assert!(
                response["result"].is_object(),
                "expected result in reply, got: {}",
                response
            );
            assert!(
                response["result"].get("terminalId").is_some(),
                "expected terminalId field, got: {}",
                response["result"]
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let envelope = Request {
                id: RequestId::Number(7),
                method: std::sync::Arc::from("terminal/output"),
                params: Some(TerminalOutputRequest::new(
                    agent_client_protocol::SessionId::from("sess-1"),
                    "term-001",
                )),
            };
            let payload = serde_json::to_vec(&envelope).unwrap();
            let reply = nats1
                .request("acp.sess-1.client.terminal.output", Bytes::from(payload))
                .await
                .expect("request must succeed");

            let response: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
            assert!(
                response.get("error").is_none(),
                "expected no error in reply, got: {}",
                response
            );
            assert!(
                response["result"].is_object(),
                "expected result in reply, got: {}",
                response
            );
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let envelope = Request {
                id: RequestId::Number(8),
                method: std::sync::Arc::from("terminal/release"),
                params: Some(ReleaseTerminalRequest::new(
                    agent_client_protocol::SessionId::from("sess-1"),
                    "term-001",
                )),
            };
            let payload = serde_json::to_vec(&envelope).unwrap();
            let reply = nats1
                .request("acp.sess-1.client.terminal.release", Bytes::from(payload))
                .await
                .expect("request must succeed");

            let response: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
            assert!(
                response.get("error").is_none(),
                "expected no error in reply, got: {}",
                response
            );
            assert!(
                response["result"].is_object(),
                "expected result in reply, got: {}",
                response
            );
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let envelope = Request {
                id: RequestId::Number(9),
                method: std::sync::Arc::from("terminal/wait_for_exit"),
                params: Some(WaitForTerminalExitRequest::new(
                    agent_client_protocol::SessionId::from("sess-1"),
                    "term-001",
                )),
            };
            let payload = serde_json::to_vec(&envelope).unwrap();
            let reply = nats1
                .request(
                    "acp.sess-1.client.terminal.wait_for_exit",
                    Bytes::from(payload),
                )
                .await
                .expect("request must succeed");

            let response: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
            assert!(
                response.get("error").is_none(),
                "expected no error in reply, got: {}",
                response
            );
            assert!(
                response["result"].is_object(),
                "expected result in reply, got: {}",
                response
            );
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            // Fire-and-forget: publish a valid PromptResponse (no reply subject expected)
            let response = PromptResponse::new(StopReason::EndTurn);
            let payload = serde_json::to_vec(&response).unwrap();
            nats1
                .publish(
                    "acp.sess-1.client.ext.session.prompt_response",
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
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats2, client_rc, bridge_rc, StdJsonSerialize).await;
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let envelope = Request {
                id: RequestId::Number(10),
                method: std::sync::Arc::from("terminal/kill"),
                params: Some(KillTerminalRequest::new(
                    agent_client_protocol::SessionId::from("sess-1"),
                    "term-001".to_string(),
                )),
            };
            let payload = serde_json::to_vec(&envelope).unwrap();
            let reply = nats1
                .request("acp.sess-1.client.terminal.kill", Bytes::from(payload))
                .await
                .expect("request must succeed");

            let response: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
            assert!(
                response.get("error").is_none(),
                "expected no error in reply, got: {}",
                response
            );
            assert!(
                response["result"].is_object(),
                "expected result in reply, got: {}",
                response
            );
        })
        .await;
}
