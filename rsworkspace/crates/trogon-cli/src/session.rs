use crate::nats::NatsClient;
use agent_client_protocol::{
    ContentBlock, NewSessionRequest, PromptRequest, SessionNotification, SessionUpdate, TextContent,
};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

const SESSION_NEW_TIMEOUT: Duration = Duration::from_secs(15);

// ── Session trait ─────────────────────────────────────────────────────────────

/// Abstraction over an ACP session. Allows injecting a mock in tests.
pub trait Session: Send + Sync + 'static {
    fn session_id(&self) -> &str;

    fn prompt(
        &self,
        text: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_;

    fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_;

    fn set_model(
        &self,
        model_id: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_;

    fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_;

    fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_;
}

// ── SessionFactory trait ──────────────────────────────────────────────────────

pub trait SessionFactory {
    type Sess: Session;

    fn create_session<'a>(
        &'a self,
        prefix: &'a str,
        cwd: PathBuf,
    ) -> impl std::future::Future<Output = anyhow::Result<Self::Sess>> + 'a;

    fn attach_session(&self, prefix: &str, session_id: String) -> Self::Sess;
}

// ── NatsSessionFactory (real implementation) ──────────────────────────────────

pub struct NatsSessionFactory<N: NatsClient + Clone> {
    nats: N,
}

impl<N: NatsClient + Clone> NatsSessionFactory<N> {
    pub fn new(nats: N) -> Self {
        Self { nats }
    }
}

impl<N: NatsClient + Clone> SessionFactory for NatsSessionFactory<N> {
    type Sess = TrogonSession<N>;

    fn create_session<'a>(
        &'a self,
        prefix: &'a str,
        cwd: PathBuf,
    ) -> impl std::future::Future<Output = anyhow::Result<TrogonSession<N>>> + 'a {
        let nats = self.nats.clone();
        let prefix = prefix.to_string();
        async move { TrogonSession::new(nats, &prefix, cwd).await }
    }

    fn attach_session(&self, prefix: &str, session_id: String) -> TrogonSession<N> {
        TrogonSession::from_existing(self.nats.clone(), prefix, session_id)
    }
}

// ── TrogonSession ─────────────────────────────────────────────────────────────

pub struct TrogonSession<N: NatsClient> {
    nats: N,
    session_id: String,
    prefix: String,
}

impl<N: NatsClient> std::fmt::Debug for TrogonSession<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrogonSession")
            .field("session_id", &self.session_id)
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl<N: NatsClient> TrogonSession<N> {
    pub fn from_existing(nats: N, prefix: &str, session_id: String) -> Self {
        Self { nats, session_id, prefix: prefix.to_string() }
    }

    pub async fn new(nats: N, prefix: &str, cwd: PathBuf) -> anyhow::Result<Self> {
        let subject = format!("{prefix}.agent.session.new");
        let req = NewSessionRequest::new(cwd);
        let payload = serde_json::to_vec(&req)?;

        let reply_bytes = tokio::time::timeout(
            SESSION_NEW_TIMEOUT,
            nats.request_bytes(subject, payload.into()),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out waiting for session creation (is trogon-acp-runner running?)"
            )
        })?
        .map_err(|e| anyhow::anyhow!("NATS error creating session: {e}"))?;

        let resp: Value = serde_json::from_slice(&reply_bytes)
            .map_err(|e| anyhow::anyhow!("invalid session response: {e}"))?;

        let session_id = resp["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("session response missing sessionId: {resp}"))?
            .to_string();

        Ok(Self { nats, session_id, prefix: prefix.to_string() })
    }
}

impl<N: NatsClient> Session for TrogonSession<N> {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn prompt(
        &self,
        text: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_
    {
        // Clone text upfront so the returned future owns it (no captured &str across awaits).
        let text = text.to_string();
        let nats = &self.nats;
        let session_id = self.session_id.clone();
        let prefix = self.prefix.clone();
        async move {
            let req_id = Uuid::now_v7().to_string();
            let notif_subject =
                format!("{prefix}.session.{session_id}.client.session.update");
            let prompt_subject =
                format!("{prefix}.session.{session_id}.agent.prompt");
            let resp_subject =
                format!("{prefix}.session.{session_id}.agent.prompt.response.{req_id}");

            let mut notif_rx = nats
                .subscribe_bytes(notif_subject)
                .await
                .map_err(|e| anyhow::anyhow!("subscribe notifications: {e}"))?;

            let mut resp_rx = nats
                .subscribe_bytes(resp_subject)
                .await
                .map_err(|e| anyhow::anyhow!("subscribe response: {e}"))?;

            let req = PromptRequest::new(
                session_id,
                vec![ContentBlock::Text(TextContent::new(&text))],
            );
            let payload = serde_json::to_vec(&req)?;

            nats.publish_with_req_id_bytes(prompt_subject, req_id, payload.into())
                .await
                .map_err(|e| anyhow::anyhow!("publish prompt: {e}"))?;

            let (tx, rx) = mpsc::channel(64);

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        bytes = resp_rx.recv() => {
                            let Some(bytes) = bytes else { break };
                            let stop = serde_json::from_slice::<Value>(&bytes)
                                .ok()
                                .and_then(|v| {
                                    v.get("stopReason")
                                        .and_then(|s| s.as_str())
                                        .map(|s| s.to_string())
                                })
                                .unwrap_or_else(|| "end_turn".to_string());
                            let _ = tx.send(StreamEvent::Done(stop)).await;
                            break;
                        }
                        bytes = notif_rx.recv() => {
                            let Some(bytes) = bytes else { break };
                            if let Ok(notif) = serde_json::from_slice::<SessionNotification>(&bytes) {
                                match notif.update {
                                    SessionUpdate::AgentMessageChunk(chunk) => {
                                        if let ContentBlock::Text(t) = chunk.content {
                                            let _ = tx.send(StreamEvent::Text(t.text)).await;
                                        }
                                    }
                                    SessionUpdate::AgentThoughtChunk(_) => {
                                        let _ = tx.send(StreamEvent::Thinking).await;
                                    }
                                    SessionUpdate::ToolCall(tc) => {
                                        if let Some(diff) =
                                            render_diff(&tc.title, tc.raw_input.as_ref())
                                        {
                                            let _ = tx.send(StreamEvent::ToolCall(tc.title.clone())).await;
                                            let _ = tx.send(StreamEvent::Diff(diff)).await;
                                        } else {
                                            let _ = tx.send(StreamEvent::ToolCall(tc.title)).await;
                                        }
                                    }
                                    SessionUpdate::UsageUpdate(u) => {
                                        let _ = tx
                                            .send(StreamEvent::Usage {
                                                used_tokens: u.used,
                                                context_size: u.size,
                                            })
                                            .await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            });

            Ok(rx)
        }
    }

    fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        let subject = format!("{}.session.{}.agent.cancel", self.prefix, self.session_id);
        let session_id = self.session_id.clone();
        async move {
            if let Ok(payload) = serde_json::to_vec(&serde_json::json!({ "sessionId": session_id })) {
                let _ = self.nats.publish_bytes(subject, payload.into()).await;
            }
        }
    }

    fn set_model(
        &self,
        model_id: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
    {
        let model_id = model_id.to_string();
        let prefix = self.prefix.clone();
        let session_id = self.session_id.clone();
        let nats = &self.nats;
        async move {
            let req_id = Uuid::now_v7().to_string();
            let subject = format!("{prefix}.session.{session_id}.agent.set_model");
            let resp_subject = format!("{prefix}.session.{session_id}.agent.set_model.response.{req_id}");

            let mut resp_rx = nats
                .subscribe_bytes(resp_subject)
                .await
                .map_err(|e| anyhow::anyhow!("subscribe set_model response: {e}"))?;

            let payload = serde_json::to_vec(&serde_json::json!({
                "sessionId": session_id,
                "modelId": model_id,
            }))?;

            nats.publish_with_req_id_bytes(subject, req_id, payload.into())
                .await
                .map_err(|e| anyhow::anyhow!("publish set_model: {e}"))?;

            tokio::time::timeout(Duration::from_secs(5), resp_rx.recv())
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for model update"))?;
            Ok(())
        }
    }

    fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
        let subject = format!("{}.compactor.compact", self.prefix);
        let session_id = self.session_id.clone();
        let nats = &self.nats;
        async move {
            let payload = serde_json::to_vec(&serde_json::json!({ "sessionId": session_id }))?;
            nats.publish_bytes(subject, payload.into())
                .await
                .map_err(|e| anyhow::anyhow!("NATS error triggering compaction: {e}"))
        }
    }

    fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        let subject = format!("{}.session.{}.agent.close", self.prefix, self.session_id);
        let session_id = self.session_id.clone();
        let prefix = self.prefix.clone();
        async move {
            let req_id = Uuid::now_v7().to_string();
            let resp_subject = format!("{prefix}.session.{session_id}.agent.close.response.{req_id}");
            // Subscribe before publishing so the runner's response is accepted (even though we ignore it).
            let _resp_rx = self.nats.subscribe_bytes(resp_subject).await;
            if let Ok(payload) = serde_json::to_vec(&serde_json::json!({ "sessionId": session_id })) {
                let _ = self.nats.publish_with_req_id_bytes(subject, req_id, payload.into()).await;
            }
        }
    }
}

// ── StreamEvent ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Text(String),
    Thinking,
    ToolCall(String),
    /// Pre-rendered colored diff for Edit/MultiEdit/Write tool calls.
    Diff(String),
    /// Token usage update at the end of a turn.
    Usage { used_tokens: u64, context_size: u64 },
    Done(String),
}

// ── Diff rendering ────────────────────────────────────────────────────────────

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

fn render_diff(tool_name: &str, input: Option<&serde_json::Value>) -> Option<String> {
    let input = input?;
    match tool_name {
        "Edit" => {
            let path = input.get("file_path")?.as_str()?;
            let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
            let new = input.get("new_string").and_then(|v| v.as_str())?;
            Some(format_edit_diff(path, old, new))
        }
        "MultiEdit" => {
            let path = input.get("file_path")?.as_str()?;
            let edits = input.get("edits")?.as_array()?;
            let mut out = format!("{DIM}--- {path}{RESET}\n{DIM}+++ {path}{RESET}");
            for edit in edits {
                let old = edit.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
                let new = edit.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
                out.push('\n');
                out.push_str(&diff_lines(old, new));
            }
            Some(out)
        }
        "Write" => {
            let path = input.get("file_path")?.as_str()?;
            let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let lines = content.lines().count();
            Some(format!("{BOLD}[write: {path}]{RESET} {DIM}({lines} lines){RESET}"))
        }
        _ => None,
    }
}

fn format_edit_diff(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("{DIM}--- {path}{RESET}\n{DIM}+++ {path}{RESET}\n");
    out.push_str(&diff_lines(old, new));
    out
}

fn diff_lines(old: &str, new: &str) -> String {
    let mut out = String::new();
    for line in old.lines() {
        out.push_str(&format!("{RED}-{line}{RESET}\n"));
    }
    for line in new.lines() {
        out.push_str(&format!("{GREEN}+{line}{RESET}\n"));
    }
    out
}

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A mock `Session` for unit tests.
    ///
    /// Pre-load a sequence of event batches with `queue_turn`. Each `prompt` call
    /// drains one batch; if none are queued, a `Done("end_turn")` is returned.
    pub struct MockSession {
        session_id: String,
        turns: Mutex<VecDeque<Vec<StreamEvent>>>,
        cancelled: Mutex<Vec<String>>,
        closed: Mutex<u32>,
        compacted: Mutex<u32>,
        model: Mutex<Option<String>>,
        set_model_error: Mutex<Option<String>>,
        compact_error: Mutex<Option<String>>,
    }

    impl MockSession {
        pub fn new(session_id: impl Into<String>) -> Self {
            Self {
                session_id: session_id.into(),
                turns: Mutex::new(VecDeque::new()),
                cancelled: Mutex::new(Vec::new()),
                closed: Mutex::new(0),
                compacted: Mutex::new(0),
                model: Mutex::new(None),
                set_model_error: Mutex::new(None),
                compact_error: Mutex::new(None),
            }
        }

        pub fn queue_turn(&self, events: Vec<StreamEvent>) {
            self.turns.lock().unwrap().push_back(events);
        }

        pub fn cancel_count(&self) -> usize {
            self.cancelled.lock().unwrap().len()
        }

        pub fn close_count(&self) -> u32 {
            *self.closed.lock().unwrap()
        }

        pub fn compact_count(&self) -> u32 {
            *self.compacted.lock().unwrap()
        }

        pub fn last_model(&self) -> Option<String> {
            self.model.lock().unwrap().clone()
        }

        pub fn fail_set_model(&self, error: impl Into<String>) {
            *self.set_model_error.lock().unwrap() = Some(error.into());
        }

        pub fn fail_compact(&self, error: impl Into<String>) {
            *self.compact_error.lock().unwrap() = Some(error.into());
        }
    }

    impl Session for MockSession {
        fn session_id(&self) -> &str {
            &self.session_id
        }

        fn prompt(
            &self,
            _text: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_
        {
            let events = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| vec![StreamEvent::Done("end_turn".into())]);
            async move {
                let (tx, rx) = mpsc::channel(events.len().max(1));
                for event in events {
                    let _ = tx.try_send(event);
                }
                Ok(rx)
            }
        }

        fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
            let id = self.session_id.clone();
            async move {
                self.cancelled.lock().unwrap().push(id);
            }
        }

        fn set_model(
            &self,
            model_id: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
        {
            let model_id = model_id.to_string();
            async move {
                if let Some(err) = self.set_model_error.lock().unwrap().clone() {
                    return Err(anyhow::anyhow!("{err}"));
                }
                *self.model.lock().unwrap() = Some(model_id);
                Ok(())
            }
        }

        fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
            async move {
                if let Some(err) = self.compact_error.lock().unwrap().clone() {
                    return Err(anyhow::anyhow!("{err}"));
                }
                *self.compacted.lock().unwrap() += 1;
                Ok(())
            }
        }

        fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
            async move {
                *self.closed.lock().unwrap() += 1;
            }
        }
    }

    impl Session for std::sync::Arc<MockSession> {
        fn session_id(&self) -> &str {
            (**self).session_id()
        }

        fn prompt(
            &self,
            text: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_
        {
            (**self).prompt(text)
        }

        fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
            (**self).cancel()
        }

        fn set_model(
            &self,
            model_id: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
        {
            (**self).set_model(model_id)
        }

        fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
            (**self).compact()
        }

        fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
            (**self).close()
        }
    }

    // ── MockSessionFactory ────────────────────────────────────────────────────

    pub struct MockSessionFactory {
        sessions: std::sync::Mutex<std::collections::VecDeque<std::sync::Arc<MockSession>>>,
        default_id: String,
    }

    impl MockSessionFactory {
        pub fn new(default_id: impl Into<String>) -> Self {
            Self {
                sessions: Default::default(),
                default_id: default_id.into(),
            }
        }

        pub fn push_session(&self, session: std::sync::Arc<MockSession>) {
            self.sessions.lock().unwrap().push_back(session);
        }
    }

    impl super::SessionFactory for MockSessionFactory {
        type Sess = std::sync::Arc<MockSession>;

        fn create_session<'a>(
            &'a self,
            _prefix: &'a str,
            _cwd: PathBuf,
        ) -> impl std::future::Future<Output = anyhow::Result<std::sync::Arc<MockSession>>> + 'a {
            async move {
                let session = self.sessions.lock().unwrap().pop_front()
                    .unwrap_or_else(|| std::sync::Arc::new(MockSession::new(&self.default_id)));
                Ok(session)
            }
        }

        fn attach_session(&self, _prefix: &str, session_id: String) -> std::sync::Arc<MockSession> {
            std::sync::Arc::new(MockSession::new(session_id))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nats::mock::MockNatsClient;
    use bytes::Bytes;
    use serde_json::json;

    // ── render_diff ───────────────────────────────────────────────────────────

    #[test]
    fn render_diff_edit_returns_colored_diff() {
        let input = json!({"file_path": "src/main.rs", "old_string": "hello", "new_string": "world"});
        let diff = render_diff("Edit", Some(&input)).unwrap();
        assert!(diff.contains("src/main.rs"), "path in header");
        assert!(diff.contains("-hello"), "old line with minus");
        assert!(diff.contains("+world"), "new line with plus");
    }

    #[test]
    fn render_diff_edit_empty_old_string() {
        let input = json!({"file_path": "new.rs", "old_string": "", "new_string": "fn main() {}"});
        let diff = render_diff("Edit", Some(&input)).unwrap();
        assert!(diff.contains("+fn main() {}"));
        assert!(!diff.contains(&format!("{RED}-")), "no removal lines for empty old_string");
    }

    #[test]
    fn render_diff_multiedit_all_edits_shown() {
        let input = json!({
            "file_path": "lib.rs",
            "edits": [
                {"old_string": "foo", "new_string": "bar"},
                {"old_string": "baz", "new_string": "qux"},
            ]
        });
        let diff = render_diff("MultiEdit", Some(&input)).unwrap();
        assert!(diff.contains("-foo") && diff.contains("+bar"));
        assert!(diff.contains("-baz") && diff.contains("+qux"));
    }

    #[test]
    fn render_diff_write_shows_path_and_line_count() {
        let content = "line1\nline2\nline3";
        let input = json!({"file_path": "out.txt", "content": content});
        let diff = render_diff("Write", Some(&input)).unwrap();
        assert!(diff.contains("out.txt"));
        assert!(diff.contains("3 lines"));
    }

    #[test]
    fn render_diff_unknown_tool_returns_none() {
        let input = json!({"file_path": "x.rs"});
        assert!(render_diff("Bash", Some(&input)).is_none());
        assert!(render_diff("Read", Some(&input)).is_none());
    }

    #[test]
    fn render_diff_none_input_returns_none() {
        assert!(render_diff("Edit", None).is_none());
    }

    // ── wire-format compatibility ─────────────────────────────────────────────

    #[test]
    fn usage_update_deserializes_from_nats_wire_format() {
        let json = json!({
            "sessionId": "sess-1",
            "update": {
                "sessionUpdate": "usage_update",
                "used": 12345,
                "size": 200000
            }
        });
        let notif: agent_client_protocol::SessionNotification =
            serde_json::from_value(json).expect("must deserialize");
        match notif.update {
            agent_client_protocol::SessionUpdate::UsageUpdate(u) => {
                assert_eq!(u.used, 12345);
                assert_eq!(u.size, 200000);
            }
            other => panic!("expected UsageUpdate, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_raw_input_is_accessible() {
        let json = json!({
            "sessionId": "sess-1",
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": "tc-1",
                "title": "Edit",
                "rawInput": {
                    "file_path": "src/lib.rs",
                    "old_string": "foo",
                    "new_string": "bar"
                }
            }
        });
        let notif: agent_client_protocol::SessionNotification =
            serde_json::from_value(json).expect("must deserialize");
        match notif.update {
            agent_client_protocol::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.title, "Edit");
                let input = tc.raw_input.expect("raw_input must be present");
                assert_eq!(input["file_path"].as_str().unwrap(), "src/lib.rs");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    // ── TrogonSession::new via MockNatsClient ─────────────────────────────────

    // ── TrogonSession::set_model ──────────────────────────────────────────────

    #[tokio::test]
    async fn set_model_sends_nats_request_and_returns_ok() {
        let nats = MockNatsClient::new();
        let resp = json!({"sessionId": "s1"});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));
        let session =
            TrogonSession::new(nats.clone(), "acp", std::path::PathBuf::from("/tmp")).await.unwrap();

        nats.queue_request_ok(Bytes::from(b"{}".as_slice()));
        let result = session.set_model("claude-opus-4-7").await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    #[tokio::test]
    async fn set_model_returns_error_on_nats_failure() {
        let nats = MockNatsClient::new();
        let resp = json!({"sessionId": "s1"});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));
        let session =
            TrogonSession::new(nats.clone(), "acp", std::path::PathBuf::from("/tmp")).await.unwrap();

        nats.queue_request_err("connection refused");
        let err = session.set_model("claude-opus-4-7").await.unwrap_err();
        assert!(err.to_string().contains("NATS error"), "got: {err}");
    }

    // ── MockSession::set_model ────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_session_set_model_stores_model() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session.set_model("claude-opus-4-7").await.unwrap();
        assert_eq!(session.last_model().as_deref(), Some("claude-opus-4-7"));
    }

    #[tokio::test]
    async fn mock_session_set_model_returns_error_when_configured() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session.fail_set_model("runner unavailable");
        let err = session.set_model("claude-opus-4-7").await.unwrap_err();
        assert!(err.to_string().contains("runner unavailable"), "got: {err}");
    }

    // ── TrogonSession::new via MockNatsClient ─────────────────────────────────

    #[tokio::test]
    async fn new_session_extracts_session_id_from_response() {
        let nats = MockNatsClient::new();
        let resp = json!({"sessionId": "test-session-42"});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));

        let session =
            TrogonSession::new(nats, "acp", std::path::PathBuf::from("/tmp")).await.unwrap();
        assert_eq!(session.session_id(), "test-session-42");
    }

    #[tokio::test]
    async fn new_session_returns_error_on_nats_failure() {
        let nats = MockNatsClient::new();
        nats.queue_request_err("connection refused");

        let err = TrogonSession::new(nats, "acp", std::path::PathBuf::from("/tmp"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("NATS error"), "got: {err}");
    }

    #[tokio::test]
    async fn new_session_returns_error_on_missing_session_id() {
        let nats = MockNatsClient::new();
        let resp = json!({"other": "field"});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));

        let err = TrogonSession::new(nats, "acp", std::path::PathBuf::from("/tmp"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("sessionId"), "got: {err}");
    }

    // ── TrogonSession::prompt via MockNatsClient ──────────────────────────────

    #[tokio::test]
    async fn prompt_streams_text_events_and_done() {
        let nats = MockNatsClient::new();
        let resp = json!({"sessionId": "s1"});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));

        // Two subscriptions: notif channel + inbox (reply) channel.
        let (notif_tx, notif_rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        let (reply_tx, reply_rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        nats.add_subscription(notif_rx);
        nats.add_subscription(reply_rx);

        let session =
            TrogonSession::new(nats, "acp", std::path::PathBuf::from("/tmp")).await.unwrap();

        let mut events_rx = session.prompt("hello").await.unwrap();

        // Send a text notification.
        let text_notif = json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "hello world"}
            }
        });
        notif_tx.send(Bytes::from(serde_json::to_vec(&text_notif).unwrap())).await.unwrap();

        // Yield so the spawned task processes the notification before we send Done.
        // Without this, the biased select! would pick the Done reply first if both
        // channels are ready simultaneously.
        tokio::task::yield_now().await;

        // Send done reply.
        let done = json!({"stopReason": "end_turn"});
        reply_tx.send(Bytes::from(serde_json::to_vec(&done).unwrap())).await.unwrap();

        let mut got_text = false;
        let mut got_done = false;
        while let Some(ev) = events_rx.recv().await {
            match ev {
                StreamEvent::Text(t) => { assert_eq!(t, "hello world"); got_text = true; }
                StreamEvent::Done(r) => { assert_eq!(r, "end_turn"); got_done = true; break; }
                _ => {}
            }
        }
        assert!(got_text, "expected Text event");
        assert!(got_done, "expected Done event");
    }

    // ── MockSession ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_session_returns_queued_events() {
        use mock::MockSession;
        let session = MockSession::new("mock-session");
        session.queue_turn(vec![
            StreamEvent::Text("hi".into()),
            StreamEvent::Done("end_turn".into()),
        ]);

        let mut rx = session.prompt("anything").await.unwrap();
        let ev1 = rx.recv().await.unwrap();
        let ev2 = rx.recv().await.unwrap();
        assert!(matches!(ev1, StreamEvent::Text(t) if t == "hi"));
        assert!(matches!(ev2, StreamEvent::Done(r) if r == "end_turn"));
    }

    #[tokio::test]
    async fn mock_session_default_turn_is_done() {
        use mock::MockSession;
        let session = MockSession::new("s");
        let mut rx = session.prompt("anything").await.unwrap();
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, StreamEvent::Done(_)));
    }

    // ── MockSession::compact ──────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_session_compact_increments_count() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session.compact().await.unwrap();
        session.compact().await.unwrap();
        assert_eq!(session.compact_count(), 2);
    }

    #[tokio::test]
    async fn mock_session_compact_returns_error_when_configured() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session.fail_compact("compactor unavailable");
        let err = session.compact().await.unwrap_err();
        assert!(err.to_string().contains("compactor unavailable"), "got: {err}");
    }

    // ── TrogonSession::compact via MockNatsClient ─────────────────────────────

    #[tokio::test]
    async fn compact_publishes_to_compactor_subject() {
        let nats = MockNatsClient::new();
        let resp = json!({"sessionId": "s1"});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));
        let session =
            TrogonSession::new(nats.clone(), "acp", std::path::PathBuf::from("/tmp")).await.unwrap();

        let result = session.compact().await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        let published = nats.published();
        assert!(
            published.iter().any(|(subj, _)| subj.contains("compactor.compact")),
            "compact subject not published; got: {published:?}"
        );
    }

    // ── NatsSessionFactory ────────────────────────────────────────────────────

    #[tokio::test]
    async fn nats_factory_create_session_returns_session_with_correct_id() {
        let nats = MockNatsClient::new();
        let resp = json!({"sessionId": "factory-created-session"});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));

        let factory = NatsSessionFactory::new(nats);
        let session = factory
            .create_session("acp", std::path::PathBuf::from("/tmp"))
            .await
            .unwrap();
        assert_eq!(session.session_id(), "factory-created-session");
    }

    #[tokio::test]
    async fn nats_factory_attach_session_returns_session_with_given_id() {
        let nats = MockNatsClient::new();
        let factory = NatsSessionFactory::new(nats);
        let session = factory.attach_session("acp", "pre-existing-id".to_string());
        assert_eq!(session.session_id(), "pre-existing-id");
    }

    // ── MockSessionFactory ────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_factory_create_returns_default_session_when_empty() {
        use mock::MockSessionFactory;
        let factory = MockSessionFactory::new("default-sess");
        let session = factory
            .create_session("acp", std::path::PathBuf::from("/tmp"))
            .await
            .unwrap();
        assert_eq!(session.session_id(), "default-sess");
    }

    #[tokio::test]
    async fn mock_factory_create_pops_queued_sessions_in_order() {
        use mock::{MockSession, MockSessionFactory};
        use std::sync::Arc;
        let factory = MockSessionFactory::new("fallback");
        factory.push_session(Arc::new(MockSession::new("first")));
        factory.push_session(Arc::new(MockSession::new("second")));

        let s1 = factory.create_session("acp", std::path::PathBuf::from("/tmp")).await.unwrap();
        let s2 = factory.create_session("acp", std::path::PathBuf::from("/tmp")).await.unwrap();
        let s3 = factory.create_session("acp", std::path::PathBuf::from("/tmp")).await.unwrap();
        assert_eq!(s1.session_id(), "first");
        assert_eq!(s2.session_id(), "second");
        assert_eq!(s3.session_id(), "fallback");
    }

    #[tokio::test]
    async fn mock_factory_attach_creates_session_with_given_id() {
        use mock::MockSessionFactory;
        let factory = MockSessionFactory::new("default");
        let session = factory.attach_session("acp", "attached-id".to_string());
        assert_eq!(session.session_id(), "attached-id");
    }
}
