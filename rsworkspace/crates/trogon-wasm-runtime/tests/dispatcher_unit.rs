/// Unit tests for `dispatcher::run` using in-memory mocks.
///
/// These tests do **not** require a real NATS server or WASM engine.
/// They verify routing logic: that the correct `Runtime` method is called
/// for each ACP subject pattern, and that replies are published back.
use agent_client_protocol::{
    ContentBlock, ContentChunk, CreateTerminalRequest, KillTerminalRequest, ReadTextFileRequest,
    ReleaseTerminalRequest, RequestPermissionOutcome, RequestPermissionRequest, SessionId,
    SessionNotification, SessionUpdate, TerminalExitStatus, TerminalId, TerminalOutputRequest,
    TextContent, ToolCallUpdate, ToolCallUpdateFields, WaitForTerminalExitRequest,
    WriteTextFileRequest,
};
use bytes::Bytes;
use futures::Stream;
use std::cell::RefCell;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use trogon_wasm_runtime::dispatcher;
use trogon_wasm_runtime::traits::{NatsBroker, Runtime};

// ── MockStream ────────────────────────────────────────────────────────────────

/// A stream backed by a tokio unbounded MPSC receiver.
///
/// Used as the subscription stream by `MockBroker`.
struct MockStream(tokio::sync::mpsc::UnboundedReceiver<async_nats::Message>);

impl Stream for MockStream {
    type Item = async_nats::Message;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

impl Unpin for MockStream {}

// ── MockBroker ────────────────────────────────────────────────────────────────

/// In-memory NATS broker for tests.
///
/// - Inject messages via `tx` (the sender half of the subscription channel).
/// - Inspect published replies via `published`.
#[derive(Clone)]
struct MockBroker {
    /// Send messages here to simulate incoming NATS messages to the dispatcher.
    pub tx: Arc<tokio::sync::mpsc::UnboundedSender<async_nats::Message>>,
    /// The receiver is taken on the first `subscribe` call.
    rx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<async_nats::Message>>>>,
    /// All messages published by the dispatcher (replies, errors, etc.).
    pub published: Arc<Mutex<Vec<(String, Bytes)>>>,
    /// The queue group name passed to the most recent `queue_subscribe` call.
    recorded_queue_group: Arc<Mutex<Option<String>>>,
}

impl MockBroker {
    fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            tx: Arc::new(tx),
            rx: Arc::new(Mutex::new(Some(rx))),
            published: Arc::new(Mutex::new(Vec::new())),
            recorded_queue_group: Arc::new(Mutex::new(None)),
        }
    }

    fn recorded_queue_group(&self) -> Option<String> {
        self.recorded_queue_group.lock().unwrap().clone()
    }

    /// Send a simulated NATS message with a reply subject.
    fn send(&self, subject: &str, reply: &str, payload: impl Into<Bytes>) {
        let payload = payload.into();
        let len = payload.len();
        let msg = async_nats::Message {
            subject: subject.into(),
            reply: Some(reply.into()),
            payload,
            headers: None,
            status: None,
            description: None,
            length: len,
        };
        self.tx.send(msg).expect("channel closed");
    }

    /// Send a simulated NATS message without a reply subject (fire-and-forget).
    fn send_no_reply(&self, subject: &str, payload: impl Into<Bytes>) {
        let payload = payload.into();
        let len = payload.len();
        let msg = async_nats::Message {
            subject: subject.into(),
            reply: None,
            payload,
            headers: None,
            status: None,
            description: None,
            length: len,
        };
        self.tx.send(msg).expect("channel closed");
    }
}

impl NatsBroker for MockBroker {
    type Sub = MockStream;

    async fn subscribe(
        &self,
        _subject: &str,
    ) -> Result<Self::Sub, Box<dyn std::error::Error + Send + Sync>> {
        let rx = self
            .rx
            .lock()
            .unwrap()
            .take()
            .expect("subscribe called more than once on MockBroker");
        Ok(MockStream(rx))
    }

    async fn publish(
        &self,
        subject: async_nats::Subject,
        payload: Bytes,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.published
            .lock()
            .unwrap()
            .push((subject.to_string(), payload));
        Ok(())
    }

    async fn request(
        &self,
        _subject: impl Into<String> + Send,
        _payload: Bytes,
    ) -> Result<async_nats::Message, Box<dyn std::error::Error + Send + Sync>> {
        Err("MockBroker does not implement request".into())
    }

    async fn queue_subscribe(
        &self,
        subject: &str,
        queue_group: &str,
    ) -> Result<Self::Sub, Box<dyn std::error::Error + Send + Sync>> {
        *self.recorded_queue_group.lock().unwrap() = Some(queue_group.to_string());
        self.subscribe(subject).await
    }
}

// ── MockRuntime ───────────────────────────────────────────────────────────────

/// Records which `Runtime` methods were called and with which arguments.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    CreateTerminal { session_id: String, command: String },
    TerminalOutput { terminal_id: String },
    KillTerminal { terminal_id: String },
    WriteToTerminal { terminal_id: String },
    CloseTerminalStdin { terminal_id: String },
    ReleaseTerminal { terminal_id: String },
    WaitForTerminalExit { terminal_id: String },
    WriteTextFile { session_id: String, path: PathBuf },
    ReadTextFile { session_id: String, path: PathBuf },
    RequestPermission,
    SessionNotification { session_id: String },
    ListSessions,
    ListTerminals,
    CleanupIdleSessions,
    CleanupAllSessions,
}

struct MockRuntime {
    pub calls: RefCell<Vec<Call>>,
}

impl MockRuntime {
    fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }
}

impl Runtime for MockRuntime {
    async fn handle_create_terminal(
        &self,
        session_id: &str,
        req: CreateTerminalRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::CreateTerminalResponse> {
        self.calls.borrow_mut().push(Call::CreateTerminal {
            session_id: session_id.to_string(),
            command: req.command.clone(),
        });
        Ok(agent_client_protocol::CreateTerminalResponse::new(
            TerminalId::new("mock-terminal"),
        ))
    }

    async fn handle_terminal_output(
        &self,
        req: TerminalOutputRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::TerminalOutputResponse> {
        self.calls.borrow_mut().push(Call::TerminalOutput {
            terminal_id: req.terminal_id.0.as_ref().to_string(),
        });
        Ok(agent_client_protocol::TerminalOutputResponse::new(
            "mock output".to_string(),
            false,
        ))
    }

    async fn handle_kill_terminal(
        &self,
        req: KillTerminalRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::KillTerminalResponse> {
        self.calls.borrow_mut().push(Call::KillTerminal {
            terminal_id: req.terminal_id.0.as_ref().to_string(),
        });
        Ok(agent_client_protocol::KillTerminalResponse::new())
    }

    async fn handle_write_to_terminal(
        &self,
        terminal_id: &str,
        _data: &[u8],
    ) -> agent_client_protocol::Result<()> {
        self.calls.borrow_mut().push(Call::WriteToTerminal {
            terminal_id: terminal_id.to_string(),
        });
        Ok(())
    }

    fn handle_close_terminal_stdin(&self, terminal_id: &str) -> agent_client_protocol::Result<()> {
        self.calls.borrow_mut().push(Call::CloseTerminalStdin {
            terminal_id: terminal_id.to_string(),
        });
        Ok(())
    }

    async fn handle_release_terminal(
        &self,
        req: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::ReleaseTerminalResponse> {
        self.calls.borrow_mut().push(Call::ReleaseTerminal {
            terminal_id: req.terminal_id.0.as_ref().to_string(),
        });
        Ok(agent_client_protocol::ReleaseTerminalResponse::new())
    }

    async fn handle_wait_for_terminal_exit(
        &self,
        req: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::WaitForTerminalExitResponse> {
        self.calls.borrow_mut().push(Call::WaitForTerminalExit {
            terminal_id: req.terminal_id.0.as_ref().to_string(),
        });
        Ok(agent_client_protocol::WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(Some(0)),
        ))
    }

    async fn handle_write_text_file(
        &self,
        session_id: &str,
        req: WriteTextFileRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::WriteTextFileResponse> {
        self.calls.borrow_mut().push(Call::WriteTextFile {
            session_id: session_id.to_string(),
            path: req.path.clone(),
        });
        Ok(agent_client_protocol::WriteTextFileResponse::new())
    }

    async fn handle_read_text_file(
        &self,
        session_id: &str,
        req: ReadTextFileRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::ReadTextFileResponse> {
        self.calls.borrow_mut().push(Call::ReadTextFile {
            session_id: session_id.to_string(),
            path: req.path.clone(),
        });
        Ok(agent_client_protocol::ReadTextFileResponse::new(
            "mock content".to_string(),
        ))
    }

    fn handle_request_permission(
        &self,
        _req: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::RequestPermissionResponse> {
        self.calls.borrow_mut().push(Call::RequestPermission);
        Ok(agent_client_protocol::RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ))
    }

    fn handle_session_notification(&self, notif: SessionNotification) {
        self.calls.borrow_mut().push(Call::SessionNotification {
            session_id: notif.session_id.0.as_ref().to_string(),
        });
    }

    fn list_sessions(&self) -> Vec<String> {
        self.calls.borrow_mut().push(Call::ListSessions);
        vec!["mock-session".to_string()]
    }

    fn list_terminals(&self) -> Vec<(String, String)> {
        self.calls.borrow_mut().push(Call::ListTerminals);
        vec![("mock-terminal".to_string(), "mock-session".to_string())]
    }

    fn cleanup_idle_sessions(&self) {
        self.calls.borrow_mut().push(Call::CleanupIdleSessions);
    }

    async fn cleanup_all_sessions(&self) {
        self.calls.borrow_mut().push(Call::CleanupAllSessions);
    }
}

// ── test helpers ──────────────────────────────────────────────────────────────

const PREFIX: &str = "test";
const SESSION: &str = "sess-123";

/// Builds the ACP client subject for a given method suffix.
fn subject(method: &str) -> String {
    format!("{PREFIX}.session.{SESSION}.client.{method}")
}

/// Runs the dispatcher with a `MockBroker` and `MockRuntime`.
///
/// The `drive` closure injects messages and drops the broker's sender when
/// done, which closes the subscription stream and causes the dispatcher to exit.
/// Returns the recorded calls and published messages.
async fn run_dispatcher<F, Fut>(drive: F) -> (Vec<Call>, Vec<(String, Bytes)>)
where
    F: FnOnce(MockBroker, Rc<MockRuntime>) -> Fut + 'static,
    Fut: std::future::Future<Output = ()>,
{
    let broker = MockBroker::new();
    let runtime = Rc::new(MockRuntime::new());

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let broker_for_drive = broker.clone();
    let runtime_for_drive = Rc::clone(&runtime);
    let broker_for_assert = broker.clone();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let broker_inner = broker.clone();
            let runtime_inner = Rc::clone(&runtime);

            // Spawn the drive closure as a local task so it can access Rc<MockRuntime>.
            let drive_task = tokio::task::spawn_local(async move {
                drive(broker_for_drive, runtime_for_drive).await;
                // Closing the sender terminates the subscription stream.
                // The dispatcher exits its loop when it receives None from sub.next().
            });

            tokio::task::spawn_local(dispatcher::run(
                broker_inner,
                PREFIX.to_string(),
                runtime_inner,
                shutdown_rx,
            ));

            // Wait for the drive task to finish, then signal shutdown.
            drive_task.await.unwrap();
            // Give the dispatcher a moment to process the last message before shutdown.
            tokio::task::yield_now().await;
            let _ = shutdown_tx.send(true);
            // Yield to let the dispatcher process shutdown.
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        })
        .await;

    let calls = runtime.calls();
    let published = broker_for_assert.published.lock().unwrap().clone();
    (calls, published)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn routes_terminal_create() {
    let req = CreateTerminalRequest::new(SessionId::from(SESSION), "test.wasm");
    let payload = serde_json::to_vec(&req).unwrap();

    let (calls, published) = run_dispatcher(|broker, _rt| async move {
        broker.send(
            &subject("terminal.create"),
            "reply.terminal.create",
            payload,
        );
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::CreateTerminal { session_id, command }
            if session_id == SESSION && command == "test.wasm")),
        "expected CreateTerminal call, got: {calls:?}"
    );
    assert!(!published.is_empty(), "expected a reply to be published");
    let reply_body: serde_json::Value =
        serde_json::from_slice(&published[0].1).expect("reply should be valid JSON");
    // ACP serializes with camelCase.
    assert!(
        reply_body.get("terminalId").is_some(),
        "reply should contain terminalId, got: {reply_body}"
    );
}

#[tokio::test]
async fn routes_terminal_output() {
    let req = TerminalOutputRequest::new(SessionId::from(SESSION), TerminalId::new("t1"));
    let payload = serde_json::to_vec(&req).unwrap();

    let (calls, published) = run_dispatcher(|broker, _rt| async move {
        broker.send(&subject("terminal.output"), "reply.output", payload);
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::TerminalOutput { terminal_id } if terminal_id == "t1")),
        "expected TerminalOutput call, got: {calls:?}"
    );
    assert!(!published.is_empty(), "expected a reply");
}

#[tokio::test]
async fn routes_terminal_kill() {
    let req = KillTerminalRequest::new(SessionId::from(SESSION), TerminalId::new("t2"));
    let payload = serde_json::to_vec(&req).unwrap();

    let (calls, _published) = run_dispatcher(|broker, _rt| async move {
        broker.send(&subject("terminal.kill"), "reply.kill", payload);
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::KillTerminal { terminal_id } if terminal_id == "t2")),
        "expected KillTerminal call, got: {calls:?}"
    );
}

#[tokio::test]
async fn routes_terminal_release() {
    let req = ReleaseTerminalRequest::new(SessionId::from(SESSION), TerminalId::new("t3"));
    let payload = serde_json::to_vec(&req).unwrap();

    let (calls, _published) = run_dispatcher(|broker, _rt| async move {
        broker.send(&subject("terminal.release"), "reply.release", payload);
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::ReleaseTerminal { terminal_id } if terminal_id == "t3")),
        "expected ReleaseTerminal call, got: {calls:?}"
    );
}

#[tokio::test]
async fn routes_terminal_wait_for_exit() {
    let req = WaitForTerminalExitRequest::new(SessionId::from(SESSION), TerminalId::new("t4"));
    let payload = serde_json::to_vec(&req).unwrap();

    let (calls, published) = run_dispatcher(|broker, _rt| async move {
        broker.send(&subject("terminal.wait_for_exit"), "reply.wait", payload);
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::WaitForTerminalExit { terminal_id } if terminal_id == "t4")),
        "expected WaitForTerminalExit call, got: {calls:?}"
    );
    assert!(!published.is_empty(), "expected a reply");
}

#[tokio::test]
async fn routes_fs_write_text_file() {
    let req = WriteTextFileRequest::new(
        SessionId::from(SESSION),
        PathBuf::from("/hello.txt"),
        "hello",
    );
    let payload = serde_json::to_vec(&req).unwrap();

    let (calls, _published) = run_dispatcher(|broker, _rt| async move {
        broker.send(&subject("fs.write_text_file"), "reply.write", payload);
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::WriteTextFile { session_id, path }
            if session_id == SESSION && path == &PathBuf::from("/hello.txt"))),
        "expected WriteTextFile call, got: {calls:?}"
    );
}

#[tokio::test]
async fn routes_fs_read_text_file() {
    let req = ReadTextFileRequest::new(SessionId::from(SESSION), PathBuf::from("/hello.txt"));
    let payload = serde_json::to_vec(&req).unwrap();

    let (calls, published) = run_dispatcher(|broker, _rt| async move {
        broker.send(&subject("fs.read_text_file"), "reply.read", payload);
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::ReadTextFile { session_id, path }
            if session_id == SESSION && path == &PathBuf::from("/hello.txt"))),
        "expected ReadTextFile call, got: {calls:?}"
    );
    assert!(!published.is_empty(), "expected a reply");
    let reply: serde_json::Value = serde_json::from_slice(&published[0].1).unwrap();
    assert_eq!(reply["content"], "mock content");
}

#[tokio::test]
async fn routes_session_request_permission() {
    let tool_call = ToolCallUpdate::new("tc-1", ToolCallUpdateFields::new());
    let req = RequestPermissionRequest::new(SessionId::from(SESSION), tool_call, vec![]);
    let payload = serde_json::to_vec(&req).unwrap();

    let (calls, _published) = run_dispatcher(|broker, _rt| async move {
        broker.send(
            &subject("session.request_permission"),
            "reply.perm",
            payload,
        );
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls.iter().any(|c| matches!(c, Call::RequestPermission)),
        "expected RequestPermission call, got: {calls:?}"
    );
}

#[tokio::test]
async fn routes_session_update_fire_and_forget() {
    let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("hi")));
    let notif = SessionNotification::new(
        SessionId::from(SESSION),
        SessionUpdate::AgentMessageChunk(chunk),
    );
    let payload = serde_json::to_vec(&notif).unwrap();

    let (calls, published) = run_dispatcher(|broker, _rt| async move {
        broker.send_no_reply(&subject("session.update"), payload);
        tokio::task::yield_now().await;
    })
    .await;

    assert!(
        calls.iter().any(
            |c| matches!(c, Call::SessionNotification { session_id } if session_id == SESSION)
        ),
        "expected SessionNotification call, got: {calls:?}"
    );
    // Fire-and-forget — no reply expected.
    assert!(
        published.is_empty(),
        "session.update should not publish a reply, but got: {published:?}"
    );
}

#[tokio::test]
async fn returns_error_for_invalid_json() {
    let (calls, published) = run_dispatcher(|broker, _rt| async move {
        broker.send(
            &subject("terminal.create"),
            "reply.bad",
            Bytes::from_static(b"not-json"),
        );
        tokio::task::yield_now().await;
    })
    .await;

    // The runtime should NOT have been called.
    assert!(
        !calls
            .iter()
            .any(|c| matches!(c, Call::CreateTerminal { .. })),
        "runtime should not be called on bad payload, got: {calls:?}"
    );
    // An error reply should have been published.
    assert!(!published.is_empty(), "expected an error reply");
    let reply: serde_json::Value = serde_json::from_slice(&published[0].1).unwrap();
    assert!(reply["error"].is_object(), "expected JSON-RPC error");
}

#[tokio::test]
async fn ignores_unparseable_subject() {
    let (calls, published) = run_dispatcher(|broker, _rt| async move {
        broker.send(
            "garbage.subject",
            "reply.garbage",
            Bytes::from_static(b"{}"),
        );
        tokio::task::yield_now().await;
    })
    .await;

    // The dispatcher calls cleanup_all_sessions on shutdown, but no routing calls.
    assert!(
        !calls
            .iter()
            .any(|c| !matches!(c, Call::CleanupAllSessions | Call::CleanupIdleSessions)),
        "no routing calls expected for unparseable subject, got: {calls:?}"
    );
    assert!(published.is_empty(), "no reply expected for bad subject");
}

#[tokio::test]
async fn dispatcher_subscribes_with_queue_group_trogon_wasm_runtime() {
    let broker = MockBroker::new();
    let runtime = Rc::new(MockRuntime::new());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let broker_clone = broker.clone();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(dispatcher::run(
                broker_clone,
                PREFIX.to_string(),
                runtime,
                shutdown_rx,
            ));
            tokio::task::yield_now().await;
            let _ = shutdown_tx.send(true);
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        })
        .await;

    assert_eq!(
        broker.recorded_queue_group(),
        Some("trogon-wasm-runtime".to_string()),
        "dispatcher must subscribe with queue group 'trogon-wasm-runtime'"
    );
}
