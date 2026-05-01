use crate::traits::{NatsBroker, Runtime};
use acp_nats::nats::{parse_client_subject, ClientMethod};
use agent_client_protocol::{
    CreateTerminalRequest, KillTerminalRequest, ReadTextFileRequest, ReleaseTerminalRequest,
    RequestPermissionRequest, SessionNotification, TerminalOutputRequest,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};
use bytes::Bytes;
use futures::StreamExt;
use std::rc::Rc;
use tracing::{debug, error, info, warn};

/// Subscribes to ACP client subjects and dispatches to a [`Runtime`].
///
/// `subject` is the full NATS subscription pattern, e.g.
/// `acp.*.session.*.client.>` — the wildcard covers all runner sub-prefixes
/// (`acp.claude`, `acp.xai`, `acp.codex`, …) so a single dispatcher instance
/// serves every runner without needing reconfiguration when runners are added.
///
/// This loop is intentionally session-aware: each message carries the
/// `session_id` in its subject, which is forwarded to every runtime handler.
/// This is the key difference from `acp_nats::client::run()`, which uses
/// the `Client` trait and loses session context by the time methods are called.
///
/// `shutdown` is a watch channel receiver; when its value becomes `true` the
/// dispatcher drains in-flight tasks and exits cleanly.
///
/// # Trust model
///
/// Session isolation is enforced at the NATS subject layer, not inside the
/// runtime. A client can only publish to subjects under its own session
/// (`{prefix}.session.{its_session_id}.client.*`) if NATS per-client ACLs
/// are configured correctly. The runtime receives messages from all sessions
/// because it is the server side of the protocol — it does not perform
/// additional session-ownership checks. This is intentional: adding a
/// redundant check here would duplicate the NATS ACL boundary and add no
/// real security. Operators must configure NATS publish permissions to
/// restrict each client to its own session subject.
pub async fn run<N, R>(
    nats: N,
    subject: String,
    runtime: Rc<R>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    N: NatsBroker,
    R: Runtime + 'static,
{
    info!(%subject, "WASM runtime dispatcher subscribing");

    let mut sub = match nats.subscribe(&subject).await {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "Failed to subscribe to client subjects");
            return;
        }
    };

    // Spawn periodic idle-session cleanup task.
    let runtime_for_cleanup = Rc::clone(&runtime);
    let mut shutdown_for_cleanup = shutdown.clone();
    tokio::task::spawn_local(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    runtime_for_cleanup.cleanup_idle_sessions();
                }
                _ = shutdown_for_cleanup.changed() => {
                    if *shutdown_for_cleanup.borrow() {
                        break;
                    }
                }
            }
        }
    });

    // Spawn periodic metrics log task (every 5 minutes).
    let mut shutdown_for_metrics = shutdown.clone();
    tokio::task::spawn_local(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let snap = crate::metrics::METRICS.snapshot();
                    tracing::info!(
                        wasm_started = snap.wasm_tasks_started,
                        wasm_completed = snap.wasm_tasks_completed,
                        wasm_faulted = snap.wasm_tasks_faulted,
                        fuel_exhausted = snap.wasm_fuel_exhausted,
                        native_started = snap.native_tasks_started,
                        cache_hits = snap.cache_hits,
                        cache_misses = snap.cache_misses,
                        host_calls = snap.host_calls_total,
                        "runtime metrics"
                    );
                }
                _ = shutdown_for_metrics.changed() => {
                    if *shutdown_for_metrics.borrow() {
                        break;
                    }
                }
            }
        }
    });

    // Track in-flight dispatch tasks.
    let mut tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    loop {
        // Reap completed tasks to avoid unbounded growth.
        while tasks.try_join_next().is_some() {}

        tokio::select! {
            biased;

            // Check for shutdown signal first.
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("Shutdown signal received in dispatcher — draining in-flight tasks");
                    break;
                }
            }

            msg = sub.next() => {
                let msg = match msg {
                    Some(m) => m,
                    None => {
                        info!("NATS subscription closed");
                        break;
                    }
                };

                let subject_str = msg.subject.to_string();
                let reply = msg.reply.clone();
                let payload = msg.payload.clone();

                let parsed = match parse_client_subject(&subject_str) {
                    Some(p) => p,
                    None => {
                        warn!(%subject_str, "Could not parse client subject — ignoring");
                        continue;
                    }
                };

                let session_id = parsed.session_id.as_str().to_string();
                let method = parsed.method;
                let runtime = Rc::clone(&runtime);
                let nats = nats.clone();

                tasks.spawn_local(async move {
                    dispatch(nats, session_id, method, payload, reply, runtime).await;
                });
            }
        }
    }

    // Drain all in-flight tasks before returning.
    info!(
        tasks = tasks.len(),
        "Waiting for in-flight tasks to complete"
    );
    tasks.join_all().await;

    // Clean up all sessions on graceful shutdown.
    runtime.cleanup_all_sessions().await;

    info!("WASM runtime dispatcher exited");
}

async fn dispatch<N, R>(
    nats: N,
    session_id: String,
    method: ClientMethod,
    payload: Bytes,
    reply: Option<async_nats::Subject>,
    runtime: Rc<R>,
) where
    N: NatsBroker,
    R: Runtime,
{
    match method {
        ClientMethod::TerminalCreate => {
            let req = match serde_json::from_slice::<CreateTerminalRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_create_terminal(&session_id, req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::TerminalOutput => {
            let req = match serde_json::from_slice::<TerminalOutputRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_terminal_output(req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::TerminalKill => {
            let req = match serde_json::from_slice::<KillTerminalRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_kill_terminal(req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::TerminalRelease => {
            let req = match serde_json::from_slice::<ReleaseTerminalRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_release_terminal(req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::TerminalWaitForExit => {
            let req = match serde_json::from_slice::<WaitForTerminalExitRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_wait_for_terminal_exit(req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::FsWriteTextFile => {
            let req = match serde_json::from_slice::<WriteTextFileRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_write_text_file(&session_id, req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::FsReadTextFile => {
            let req = match serde_json::from_slice::<ReadTextFileRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_read_text_file(&session_id, req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::SessionRequestPermission => {
            let req = match serde_json::from_slice::<RequestPermissionRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_request_permission(req);
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::SessionUpdate => {
            // Fire-and-forget notification — no reply expected.
            match serde_json::from_slice::<SessionNotification>(&payload) {
                Ok(notif) => runtime.handle_session_notification(notif),
                Err(e) => warn!(error = %e, "Failed to deserialize SessionNotification"),
            }
        }

        ClientMethod::ExtSessionPromptResponse => {
            debug!("ExtSessionPromptResponse — not handled");
            reply_error(&nats, reply, -32601, "Method not supported by this runtime").await;
        }

        ClientMethod::Ext(ref name) if name == "terminal.write_stdin" => {
            // Payload: { "terminal_id": "...", "data": [1, 2, 3, ...] }
            #[derive(serde::Deserialize)]
            struct WriteStdinRequest {
                terminal_id: String,
                /// Data as a JSON array of bytes, e.g. [104, 101, 108, 108, 111].
                data: Vec<u8>,
            }
            match serde_json::from_slice::<WriteStdinRequest>(&payload) {
                Ok(req) => {
                    let result = runtime
                        .handle_write_to_terminal(&req.terminal_id, &req.data)
                        .await;
                    reply_result(&nats, reply, result).await;
                }
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                }
            }
        }

        ClientMethod::Ext(ref name) if name == "terminal.close_stdin" => {
            // Payload: { "terminal_id": "..." }
            #[derive(serde::Deserialize)]
            struct CloseStdinRequest {
                terminal_id: String,
            }
            match serde_json::from_slice::<CloseStdinRequest>(&payload) {
                Ok(req) => {
                    let result = runtime.handle_close_terminal_stdin(&req.terminal_id);
                    reply_result(&nats, reply, result).await;
                }
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                }
            }
        }

        ClientMethod::Ext(ref name) if name == "runtime.list_sessions" => {
            let sessions = runtime.list_sessions();
            let body = serde_json::json!({ "sessions": sessions });
            if let Some(reply_to) = reply {
                if let Ok(b) = serde_json::to_vec(&body) {
                    if let Err(e) = nats.publish(reply_to, b.into()).await {
                        warn!(error = %e, "Failed to publish list_sessions reply");
                    }
                }
            }
        }

        ClientMethod::Ext(ref name) if name == "runtime.list_terminals" => {
            let terminals = runtime.list_terminals();
            let items: Vec<serde_json::Value> = terminals
                .into_iter()
                .map(|(tid, sid)| serde_json::json!({ "terminal_id": tid, "session_id": sid }))
                .collect();
            let body = serde_json::json!({ "terminals": items });
            if let Some(reply_to) = reply {
                if let Ok(b) = serde_json::to_vec(&body) {
                    if let Err(e) = nats.publish(reply_to, b.into()).await {
                        warn!(error = %e, "Failed to publish list_terminals reply");
                    }
                }
            }
        }

        ClientMethod::Ext(_) => {
            debug!("Unhandled Ext method");
            reply_error(&nats, reply, -32601, "Method not supported by this runtime").await;
        }
    }
}

async fn reply_result<N, T>(
    nats: &N,
    reply: Option<async_nats::Subject>,
    result: agent_client_protocol::Result<T>,
) where
    N: NatsBroker,
    T: serde::Serialize,
{
    let Some(reply_to) = reply else { return };

    let body = match result {
        Ok(resp) => match serde_json::to_vec(&resp) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "Failed to serialize response");
                return;
            }
        },
        Err(err) => {
            let json = serde_json::json!({
                "jsonrpc": "2.0",
                "error": { "code": err.code, "message": err.message }
            });
            match serde_json::to_vec(&json) {
                Ok(b) => b,
                Err(e) => {
                    error!(error = %e, "Failed to serialize error response");
                    return;
                }
            }
        }
    };

    if let Err(e) = nats.publish(reply_to, body.into()).await {
        warn!(error = %e, "Failed to publish reply");
    }
}

async fn reply_error<N: NatsBroker>(
    nats: &N,
    reply: Option<async_nats::Subject>,
    code: i64,
    message: &str,
) {
    let Some(reply_to) = reply else { return };
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "error": { "code": code, "message": message }
    });
    if let Ok(b) = serde_json::to_vec(&body) {
        if let Err(e) = nats.publish(reply_to, b.into()).await {
            warn!(error = %e, "Failed to publish error reply");
        }
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use acp_nats::nats::ClientMethod;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, CreateTerminalResponse, KillTerminalResponse,
        ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalResponse,
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        SessionNotification, SessionUpdate, TerminalExitStatus, TerminalId,
        TerminalOutputResponse, TerminalOutputRequest, WaitForTerminalExitResponse,
        WriteTextFileRequest, WriteTextFileResponse,
    };
    use bytes::Bytes;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    // ── RecordingBroker ───────────────────────────────────────────────────────

    /// Captures every `publish` call; `subscribe` returns an empty stream so
    /// the dispatcher loop never blocks waiting for messages.
    #[derive(Clone)]
    struct RecordingBroker {
        published: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    }

    impl RecordingBroker {
        fn new() -> Self {
            Self { published: Arc::new(Mutex::new(vec![])) }
        }

        fn published(&self) -> Vec<(String, Vec<u8>)> {
            self.published.lock().unwrap().clone()
        }

        fn first_reply_json(&self) -> serde_json::Value {
            let msgs = self.published();
            assert!(!msgs.is_empty(), "expected at least one published message");
            serde_json::from_slice(&msgs[0].1).expect("reply is valid JSON")
        }
    }

    impl NatsBroker for RecordingBroker {
        type Sub = futures::stream::Empty<async_nats::Message>;

        async fn subscribe(
            &self,
            _: &str,
        ) -> Result<Self::Sub, Box<dyn std::error::Error + Send + Sync>> {
            Ok(futures::stream::empty())
        }

        async fn publish(
            &self,
            subject: async_nats::Subject,
            payload: bytes::Bytes,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.published
                .lock()
                .unwrap()
                .push((subject.to_string(), payload.to_vec()));
            Ok(())
        }

        async fn request(
            &self,
            _: impl Into<String> + Send,
            _: bytes::Bytes,
        ) -> Result<async_nats::Message, Box<dyn std::error::Error + Send + Sync>> {
            Err("recording broker: request not supported".into())
        }
    }

    // ── MockRuntime ───────────────────────────────────────────────────────────

    /// Records every method invocation as `"method_name:arg"` strings so tests
    /// can assert on routing and session-id forwarding without touching real FS
    /// or processes.
    struct MockRuntime {
        calls:     RefCell<Vec<String>>,
        sessions:  Vec<String>,
        terminals: Vec<(String, String)>,
    }

    impl MockRuntime {
        fn new() -> Self {
            Self { calls: RefCell::new(vec![]), sessions: vec![], terminals: vec![] }
        }

        fn with_sessions(sessions: Vec<String>) -> Self {
            Self { calls: RefCell::new(vec![]), sessions, terminals: vec![] }
        }

        fn with_terminals(terminals: Vec<(String, String)>) -> Self {
            Self { calls: RefCell::new(vec![]), sessions: vec![], terminals }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl Runtime for MockRuntime {
        async fn handle_create_terminal(
            &self,
            session_id: &str,
            _: agent_client_protocol::CreateTerminalRequest,
        ) -> agent_client_protocol::Result<CreateTerminalResponse> {
            self.calls.borrow_mut().push(format!("create_terminal:{session_id}"));
            Ok(CreateTerminalResponse::new(TerminalId::new("mock-tid")))
        }

        async fn handle_terminal_output(
            &self,
            req: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            self.calls
                .borrow_mut()
                .push(format!("terminal_output:{}", req.terminal_id.0.as_ref()));
            Ok(TerminalOutputResponse::new("", false))
        }

        async fn handle_kill_terminal(
            &self,
            req: agent_client_protocol::KillTerminalRequest,
        ) -> agent_client_protocol::Result<KillTerminalResponse> {
            self.calls
                .borrow_mut()
                .push(format!("kill_terminal:{}", req.terminal_id.0.as_ref()));
            Ok(KillTerminalResponse::new())
        }

        async fn handle_write_to_terminal(
            &self,
            terminal_id: &str,
            _: &[u8],
        ) -> agent_client_protocol::Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("write_to_terminal:{terminal_id}"));
            Ok(())
        }

        fn handle_close_terminal_stdin(
            &self,
            terminal_id: &str,
        ) -> agent_client_protocol::Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("close_stdin:{terminal_id}"));
            Ok(())
        }

        async fn handle_release_terminal(
            &self,
            req: agent_client_protocol::ReleaseTerminalRequest,
        ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
            self.calls
                .borrow_mut()
                .push(format!("release_terminal:{}", req.terminal_id.0.as_ref()));
            Ok(ReleaseTerminalResponse::new())
        }

        async fn handle_wait_for_terminal_exit(
            &self,
            req: agent_client_protocol::WaitForTerminalExitRequest,
        ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
            self.calls
                .borrow_mut()
                .push(format!("wait_for_exit:{}", req.terminal_id.0.as_ref()));
            Ok(WaitForTerminalExitResponse::new(TerminalExitStatus::new()))
        }

        async fn handle_write_text_file(
            &self,
            session_id: &str,
            req: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
            self.calls.borrow_mut().push(format!(
                "write_text_file:{session_id}:{}",
                req.path.display()
            ));
            Ok(WriteTextFileResponse::new())
        }

        async fn handle_read_text_file(
            &self,
            session_id: &str,
            req: ReadTextFileRequest,
        ) -> agent_client_protocol::Result<ReadTextFileResponse> {
            self.calls.borrow_mut().push(format!(
                "read_text_file:{session_id}:{}",
                req.path.display()
            ));
            Ok(ReadTextFileResponse::new("mock content"))
        }

        fn handle_request_permission(
            &self,
            _: RequestPermissionRequest,
        ) -> agent_client_protocol::Result<RequestPermissionResponse> {
            self.calls.borrow_mut().push("request_permission".into());
            Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
        }

        fn handle_session_notification(&self, notif: SessionNotification) {
            self.calls
                .borrow_mut()
                .push(format!("session_notification:{}", notif.session_id.0.as_ref()));
        }

        fn list_sessions(&self) -> Vec<String> {
            self.sessions.clone()
        }

        fn list_terminals(&self) -> Vec<(String, String)> {
            self.terminals.clone()
        }

        fn cleanup_idle_sessions(&self) {
            self.calls.borrow_mut().push("cleanup_idle_sessions".into());
        }

        async fn cleanup_all_sessions(&self) {
            self.calls.borrow_mut().push("cleanup_all_sessions".into());
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn reply(s: &str) -> Option<async_nats::Subject> {
        Some(async_nats::Subject::from(s))
    }

    fn json_bytes(v: &serde_json::Value) -> Bytes {
        Bytes::from(serde_json::to_vec(v).unwrap())
    }

    fn error_code(body: &serde_json::Value) -> i64 {
        body["error"]["code"].as_i64().expect("error.code must be present")
    }

    // ── Invalid-JSON parse errors (code -32600) ───────────────────────────────

    #[tokio::test]
    async fn write_text_file_invalid_json_replies_32600() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(), ClientMethod::FsWriteTextFile,
            Bytes::from_static(b"not json"), reply("reply.to"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32600);
    }

    #[tokio::test]
    async fn read_text_file_invalid_json_replies_32600() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(), ClientMethod::FsReadTextFile,
            Bytes::from_static(b"{}invalid"), reply("reply.to"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32600);
    }

    #[tokio::test]
    async fn terminal_create_invalid_json_replies_32600() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(), ClientMethod::TerminalCreate,
            Bytes::from_static(b"bad"), reply("reply.to"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32600);
    }

    #[tokio::test]
    async fn terminal_kill_invalid_json_replies_32600() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(), ClientMethod::TerminalKill,
            Bytes::from_static(b"bad"), reply("reply.to"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32600);
    }

    #[tokio::test]
    async fn request_permission_invalid_json_replies_32600() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(), ClientMethod::SessionRequestPermission,
            Bytes::from_static(b"bad"), reply("reply.to"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32600);
    }

    #[tokio::test]
    async fn write_stdin_invalid_json_replies_32600() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(),
            ClientMethod::Ext("terminal.write_stdin".into()),
            Bytes::from_static(b"not json"), reply("reply.to"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32600);
    }

    #[tokio::test]
    async fn close_stdin_invalid_json_replies_32600() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(),
            ClientMethod::Ext("terminal.close_stdin".into()),
            Bytes::from_static(b"bad"), reply("reply.to"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32600);
    }

    // ── Routing and session-id forwarding ─────────────────────────────────────

    #[tokio::test]
    async fn write_text_file_forwards_session_id() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = serde_json::to_vec(
            &WriteTextFileRequest::new("s1", PathBuf::from("/f.txt"), "hi")
        ).unwrap();
        dispatch(broker, "my-session".into(), ClientMethod::FsWriteTextFile,
            Bytes::from(payload), reply("r"), Rc::clone(&rt)).await;
        assert!(rt.calls()[0].starts_with("write_text_file:my-session:"),
            "session_id must come from the subject, not the request body; got {:?}", rt.calls());
    }

    #[tokio::test]
    async fn read_text_file_forwards_session_id() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = serde_json::to_vec(
            &ReadTextFileRequest::new("s1", PathBuf::from("/f.txt"))
        ).unwrap();
        dispatch(broker, "other-session".into(), ClientMethod::FsReadTextFile,
            Bytes::from(payload), reply("r"), Rc::clone(&rt)).await;
        assert!(rt.calls()[0].starts_with("read_text_file:other-session:"),
            "session_id must come from the subject; got {:?}", rt.calls());
    }

    #[tokio::test]
    async fn terminal_create_routes_with_session_id() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = json_bytes(&serde_json::json!({"sessionId": "s1", "command": "echo"}));
        dispatch(broker, "sess42".into(), ClientMethod::TerminalCreate,
            payload, reply("r"), Rc::clone(&rt)).await;
        assert_eq!(rt.calls()[0], "create_terminal:sess42");
    }

    #[tokio::test]
    async fn terminal_kill_routes_to_runtime() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = json_bytes(&serde_json::json!({"sessionId": "s1", "terminalId": "tid1"}));
        dispatch(broker, "s1".into(), ClientMethod::TerminalKill,
            payload, reply("r"), Rc::clone(&rt)).await;
        assert_eq!(rt.calls()[0], "kill_terminal:tid1");
    }

    #[tokio::test]
    async fn terminal_output_routes_to_runtime() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = json_bytes(&serde_json::json!({"sessionId": "s1", "terminalId": "tid2"}));
        dispatch(broker, "s1".into(), ClientMethod::TerminalOutput,
            payload, reply("r"), Rc::clone(&rt)).await;
        assert_eq!(rt.calls()[0], "terminal_output:tid2");
    }

    #[tokio::test]
    async fn terminal_release_routes_to_runtime() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = json_bytes(&serde_json::json!({"sessionId": "s1", "terminalId": "tid3"}));
        dispatch(broker, "s1".into(), ClientMethod::TerminalRelease,
            payload, reply("r"), Rc::clone(&rt)).await;
        assert_eq!(rt.calls()[0], "release_terminal:tid3");
    }

    #[tokio::test]
    async fn terminal_wait_for_exit_routes_to_runtime() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = json_bytes(&serde_json::json!({"sessionId": "s1", "terminalId": "tid4"}));
        dispatch(broker, "s1".into(), ClientMethod::TerminalWaitForExit,
            payload, reply("r"), Rc::clone(&rt)).await;
        assert_eq!(rt.calls()[0], "wait_for_exit:tid4");
    }

    #[tokio::test]
    async fn request_permission_routes_to_runtime() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = json_bytes(&serde_json::json!({
            "sessionId": "s1",
            "toolCall": {"toolCallId": "tc1"},
            "options": []
        }));
        dispatch(broker, "s1".into(), ClientMethod::SessionRequestPermission,
            payload, reply("r"), Rc::clone(&rt)).await;
        assert_eq!(rt.calls()[0], "request_permission");
    }

    #[tokio::test]
    async fn write_stdin_routes_to_runtime() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = json_bytes(&serde_json::json!({"terminal_id": "tid5", "data": [104, 105]}));
        dispatch(broker, "s1".into(),
            ClientMethod::Ext("terminal.write_stdin".into()),
            payload, reply("r"), Rc::clone(&rt)).await;
        assert_eq!(rt.calls()[0], "write_to_terminal:tid5");
    }

    #[tokio::test]
    async fn close_stdin_routes_to_runtime() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = json_bytes(&serde_json::json!({"terminal_id": "tid6"}));
        dispatch(broker, "s1".into(),
            ClientMethod::Ext("terminal.close_stdin".into()),
            payload, reply("r"), Rc::clone(&rt)).await;
        assert_eq!(rt.calls()[0], "close_stdin:tid6");
    }

    // ── Session update (fire-and-forget, no reply) ─────────────────────────────

    #[tokio::test]
    async fn session_update_calls_runtime_and_publishes_nothing() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let notif = SessionNotification::new(
            "sess99",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::Text(agent_client_protocol::TextContent::new("hi")),
            )),
        );
        let payload = Bytes::from(serde_json::to_vec(&notif).unwrap());
        dispatch(broker.clone(), "sess99".into(), ClientMethod::SessionUpdate,
            payload, reply("r"), Rc::clone(&rt)).await;
        assert!(broker.published().is_empty(), "session_update must not publish a reply");
        assert_eq!(rt.calls()[0], "session_notification:sess99");
    }

    #[tokio::test]
    async fn session_update_invalid_json_warns_and_does_not_publish() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(), ClientMethod::SessionUpdate,
            Bytes::from_static(b"bad json"), reply("r"), rt).await;
        assert!(broker.published().is_empty(), "invalid session_update must not reply");
    }

    // ── No reply subject → nothing published ──────────────────────────────────

    #[tokio::test]
    async fn no_reply_subject_produces_no_publish() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        let payload = Bytes::from(serde_json::to_vec(
            &WriteTextFileRequest::new("s1", PathBuf::from("/f.txt"), "x")
        ).unwrap());
        dispatch(broker.clone(), "s1".into(), ClientMethod::FsWriteTextFile,
            payload, None, Rc::clone(&rt)).await;
        assert!(broker.published().is_empty(), "no reply subject → no publish");
        assert_eq!(rt.calls()[0], "write_text_file:s1:/f.txt");
    }

    // ── Extension method — unsupported Ext → -32601 ───────────────────────────

    #[tokio::test]
    async fn unknown_ext_method_replies_32601() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(),
            ClientMethod::Ext("no.such.method".into()),
            Bytes::new(), reply("r"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32601);
    }

    #[tokio::test]
    async fn ext_session_prompt_response_replies_32601() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::new());
        dispatch(broker.clone(), "s1".into(), ClientMethod::ExtSessionPromptResponse,
            Bytes::new(), reply("r"), rt).await;
        assert_eq!(error_code(&broker.first_reply_json()), -32601);
    }

    // ── Extension methods — runtime.list_sessions / list_terminals ────────────

    #[tokio::test]
    async fn list_sessions_ext_returns_sessions_json() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::with_sessions(vec![
            "alice".into(), "bob".into(),
        ]));
        dispatch(broker.clone(), "s1".into(),
            ClientMethod::Ext("runtime.list_sessions".into()),
            Bytes::new(), reply("reply.subj"), rt).await;
        let body = broker.first_reply_json();
        let sessions: Vec<String> = serde_json::from_value(body["sessions"].clone()).unwrap();
        assert!(sessions.contains(&"alice".to_string()));
        assert!(sessions.contains(&"bob".to_string()));
    }

    #[tokio::test]
    async fn list_terminals_ext_returns_terminals_json() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::with_terminals(vec![
            ("tid-a".into(), "sess-a".into()),
        ]));
        dispatch(broker.clone(), "s1".into(),
            ClientMethod::Ext("runtime.list_terminals".into()),
            Bytes::new(), reply("reply.subj"), rt).await;
        let body = broker.first_reply_json();
        let items = body["terminals"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["terminal_id"], "tid-a");
        assert_eq!(items[0]["session_id"], "sess-a");
    }

    #[tokio::test]
    async fn list_sessions_no_reply_subject_does_not_publish() {
        let broker = RecordingBroker::new();
        let rt = Rc::new(MockRuntime::with_sessions(vec!["s1".into()]));
        dispatch(broker.clone(), "s1".into(),
            ClientMethod::Ext("runtime.list_sessions".into()),
            Bytes::new(), None, rt).await;
        assert!(broker.published().is_empty());
    }
}
