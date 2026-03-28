use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{
    AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    CloseSessionRequest, CloseSessionResponse, ContentBlock, ContentChunk, Error, ErrorCode,
    ForkSessionRequest, ForkSessionResponse, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, ModelInfo,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, ProtocolVersion,
    ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionCloseCapabilities,
    SessionForkCapabilities, SessionId, SessionInfo, SessionListCapabilities, SessionMode,
    SessionModeState, SessionModelState, SessionNotification, SessionResumeCapabilities,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse,
    StopReason, ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::Client as _;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use crate::process::{CodexEvent, CodexProcess};

/// Holds the process mutex lock and derefs directly to `CodexProcess`,
/// encoding the post-condition of `process()` (always `Some`) in the type
/// instead of requiring callers to call `.as_ref().unwrap()`.
struct ProcessGuard(tokio::sync::OwnedMutexGuard<Option<CodexProcess>>);

impl std::ops::Deref for ProcessGuard {
    type Target = CodexProcess;
    fn deref(&self) -> &CodexProcess {
        self.0.as_ref().expect("CodexProcess guaranteed present by process()")
    }
}

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

struct CodexSession {
    thread_id: String,
    cwd: String,
    /// Per-session model override. None means use the agent default.
    model: Option<String>,
}

/// ACP Agent implementation backed by a `codex app-server` subprocess.
///
/// Each `CodexAgent` manages one `codex app-server` process. ACP sessions map
/// to Codex threads (thread_id). Prompt calls run Codex turns and stream the
/// resulting events back to the ACP client as `SessionNotification`s via NATS.
///
/// The subprocess is spawned lazily and re-spawned automatically if it crashes.
/// Re-spawning clears all in-memory sessions since Codex thread state is lost.
pub struct CodexAgent {
    nats: async_nats::Client,
    acp_prefix: AcpPrefix,
    process: Arc<Mutex<Option<CodexProcess>>>,
    sessions: Arc<Mutex<HashMap<String, CodexSession>>>,
    default_model: String,
    prompt_timeout: Duration,
    available_models: Vec<ModelInfo>,
}

impl CodexAgent {
    /// Create a new `CodexAgent`. The `codex app-server` process is spawned
    /// lazily on the first call that needs it.
    ///
    /// Environment variables read once at construction:
    /// - `CODEX_PROMPT_TIMEOUT_SECS` — prompt timeout in seconds (default: 7200)
    /// - `CODEX_MODELS` — comma-separated `id:label` pairs (default: `o4-mini:o4-mini,o3:o3,gpt-4o:GPT-4o`)
    pub fn new(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
    ) -> Self {
        let default_model: String = default_model.into();
        let prompt_timeout = std::env::var("CODEX_PROMPT_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(7200));

        let mut available_models = std::env::var("CODEX_MODELS")
            .ok()
            .map(|s| {
                s.split(',')
                    .filter_map(|entry| {
                        match entry.split_once(':') {
                            Some((id, label)) => Some(ModelInfo::new(
                                id.trim().to_string(),
                                label.trim().to_string(),
                            )),
                            None => {
                                warn!(entry, "CODEX_MODELS: skipping malformed entry (expected 'id:label')");
                                None
                            }
                        }
                    })
                    .collect()
            })
            .filter(|v: &Vec<ModelInfo>| !v.is_empty())
            .unwrap_or_else(|| {
                vec![
                    ModelInfo::new("o4-mini", "o4-mini"),
                    ModelInfo::new("o3", "o3"),
                    ModelInfo::new("gpt-4o", "GPT-4o"),
                ]
            });

        // Ensure the default model appears in the list so session_model_state
        // never reports a current model that the client cannot select.
        if !available_models.iter().any(|m| m.model_id.0.as_ref() == default_model.as_str()) {
            warn!(model = %default_model, "default model not in available list; adding it");
            available_models.push(ModelInfo::new(default_model.clone(), default_model.clone()));
        }

        Self {
            nats,
            acp_prefix,
            process: Arc::new(Mutex::new(None)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            default_model,
            prompt_timeout,
            available_models,
        }
    }

    /// Ensures the `CodexProcess` is running, spawning (or re-spawning) as needed,
    /// and returns a guard that holds the mutex and derefs to `&CodexProcess`.
    ///
    /// If the previous process has exited, in-memory sessions are cleared —
    /// Codex thread state is stored in the subprocess, so they cannot be recovered.
    async fn process(&self) -> Result<ProcessGuard, Error> {
        let mut guard = Arc::clone(&self.process).lock_owned().await;
        let needs_spawn = guard.as_ref().is_none_or(|p| !p.is_alive());
        if needs_spawn {
            if guard.is_some() {
                warn!("codex app-server exited; re-spawning (existing sessions invalidated)");
                self.sessions.lock().await.clear();
            }
            match CodexProcess::spawn().await {
                Ok(p) => *guard = Some(p),
                Err(e) => {
                    return Err(internal_error(format!("failed to spawn codex app-server: {e}")));
                }
            }
        }
        Ok(ProcessGuard(guard))
    }

    fn make_nats_client(
        &self,
        session_id: &SessionId,
    ) -> agent_client_protocol::Result<NatsClientProxy<async_nats::Client>> {
        let acp_session_id =
            AcpSessionId::try_from(session_id).map_err(|e| internal_error(e.to_string()))?;
        Ok(NatsClientProxy::new(
            self.nats.clone(),
            acp_session_id,
            self.acp_prefix.clone(),
            Duration::from_secs(30),
        ))
    }

    fn session_mode_state(&self) -> SessionModeState {
        // Codex does not have named permission modes — expose a single "default".
        SessionModeState::new(
            "default".to_string(),
            vec![SessionMode::new("default", "Default")],
        )
    }

    fn session_model_state(&self, current: Option<&str>) -> SessionModelState {
        let current = current.unwrap_or(&self.default_model).to_string();
        SessionModelState::new(current, self.available_models.clone())
    }
}

#[async_trait(?Send)]
impl agent_client_protocol::Agent for CodexAgent {
    async fn initialize(
        &self,
        _req: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        Ok(InitializeResponse::new(ProtocolVersion::LATEST)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .session_capabilities(
                        SessionCapabilities::new()
                            .fork(SessionForkCapabilities::new())
                            .list(SessionListCapabilities::new())
                            .resume(SessionResumeCapabilities::new())
                            .close(SessionCloseCapabilities::new()),
                    ),
            )
            .agent_info(Implementation::new(
                "trogon-codex-runner",
                env!("CARGO_PKG_VERSION"),
            )))
    }

    async fn authenticate(
        &self,
        _req: AuthenticateRequest,
    ) -> agent_client_protocol::Result<AuthenticateResponse> {
        // Codex uses the OPENAI_API_KEY env var — no per-session auth needed.
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(
        &self,
        req: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        let cwd = req.cwd.to_string_lossy().into_owned();

        let proc = self.process().await?;
        let thread_id = proc.thread_start(&cwd).await.map_err(|e| internal_error(e.to_string()))?;
        drop(proc); // release process lock before acquiring sessions lock

        let session_id = Uuid::new_v4().to_string();
        self.sessions.lock().await.insert(
            session_id.clone(),
            CodexSession { thread_id, cwd, model: None },
        );

        info!(session_id, "codex: new session");
        Ok(NewSessionResponse::new(SessionId::from(session_id))
            .modes(self.session_mode_state())
            .models(self.session_model_state(None)))
    }

    async fn load_session(
        &self,
        req: LoadSessionRequest,
    ) -> agent_client_protocol::Result<LoadSessionResponse> {
        let session_id = req.session_id.to_string();

        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(&session_id) {
            let model = session.model.as_deref();
            return Ok(LoadSessionResponse::new()
                .modes(self.session_mode_state())
                .models(self.session_model_state(model)));
        }

        Err(internal_error(format!("session {session_id} not found")))
    }

    async fn resume_session(
        &self,
        req: ResumeSessionRequest,
    ) -> agent_client_protocol::Result<ResumeSessionResponse> {
        let session_id = req.session_id.to_string();

        let thread_id = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&session_id)
                .map(|s| s.thread_id.clone())
                .ok_or_else(|| internal_error(format!("session {session_id} not found")))?
        };

        let proc = self.process().await?;
        // thread/resume is a best-effort hint to Codex; the thread stays alive
        // in the subprocess regardless, so a failure here is non-fatal.
        if let Err(e) = proc.thread_resume(&thread_id).await {
            warn!(session_id, error = %e, "codex: thread_resume failed (non-fatal)");
        }

        Ok(ResumeSessionResponse::new())
    }

    async fn fork_session(
        &self,
        req: ForkSessionRequest,
    ) -> agent_client_protocol::Result<ForkSessionResponse> {
        let source_id = req.session_id.to_string();
        let cwd = req.cwd.to_string_lossy().into_owned();

        let (source_thread_id, inherited_model) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&source_id)
                .ok_or_else(|| internal_error(format!("session {source_id} not found")))?;
            (s.thread_id.clone(), s.model.clone())
        };

        let proc = self.process().await?;
        let new_thread_id =
            proc.thread_fork(&source_thread_id).await.map_err(|e| internal_error(e.to_string()))?;
        drop(proc); // release process lock before acquiring sessions lock

        let new_session_id = Uuid::new_v4().to_string();
        self.sessions.lock().await.insert(
            new_session_id.clone(),
            CodexSession { thread_id: new_thread_id, cwd, model: inherited_model.clone() },
        );

        Ok(ForkSessionResponse::new(new_session_id)
            .modes(self.session_mode_state())
            .models(self.session_model_state(inherited_model.as_deref())))
    }

    async fn close_session(
        &self,
        req: CloseSessionRequest,
    ) -> agent_client_protocol::Result<CloseSessionResponse> {
        let session_id = req.session_id.to_string();
        self.sessions.lock().await.remove(&session_id);
        // Codex app-server has no thread/close method — the subprocess retains
        // thread state in memory until it exits or is re-spawned.
        info!(session_id, "codex: session closed");
        Ok(CloseSessionResponse::new())
    }

    async fn list_sessions(
        &self,
        _req: ListSessionsRequest,
    ) -> agent_client_protocol::Result<ListSessionsResponse> {
        let sessions = self.sessions.lock().await;
        let mut list: Vec<_> = sessions
            .iter()
            .map(|(id, s)| SessionInfo::new(id.clone(), s.cwd.clone()))
            .collect();
        list.sort_by(|a, b| a.session_id.0.cmp(&b.session_id.0));
        Ok(ListSessionsResponse::new(list))
    }

    async fn set_session_mode(
        &self,
        _req: SetSessionModeRequest,
    ) -> agent_client_protocol::Result<SetSessionModeResponse> {
        // Codex does not have ACP permission modes — silently accept.
        Ok(SetSessionModeResponse::new())
    }

    async fn set_session_model(
        &self,
        req: SetSessionModelRequest,
    ) -> agent_client_protocol::Result<SetSessionModelResponse> {
        let session_id = req.session_id.to_string();
        let model_id = req.model_id.to_string();

        if !self.available_models.iter().any(|m| m.model_id.0.as_ref() == model_id) {
            return Err(internal_error(format!("unknown model: {model_id}")));
        }

        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(&session_id) {
            Some(session) => {
                session.model = Some(model_id.clone());
                info!(session_id, model = %model_id, "codex: set_session_model");
                Ok(SetSessionModelResponse::new())
            }
            None => Err(internal_error(format!("session {session_id} not found"))),
        }
    }

    async fn set_session_config_option(
        &self,
        _req: SetSessionConfigOptionRequest,
    ) -> agent_client_protocol::Result<SetSessionConfigOptionResponse> {
        Ok(SetSessionConfigOptionResponse::new(vec![]))
    }

    async fn prompt(
        &self,
        req: PromptRequest,
    ) -> agent_client_protocol::Result<PromptResponse> {
        let session_id = req.session_id.to_string();
        let nats_client = self.make_nats_client(&req.session_id)?;

        // Extract plain text from the prompt content blocks.
        // Non-text blocks (images, tool results) are not supported by Codex and are dropped.
        let user_input: String = req
            .prompt
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        if user_input.is_empty() {
            warn!(session_id, "codex: prompt contains no text blocks; sending empty input to Codex");
        }

        let (thread_id, model) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&session_id)
                .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
            (s.thread_id.clone(), s.model.clone())
        };

        let proc = self.process().await?;
        let mut event_rx = proc
            .turn_start(&thread_id, &user_input, model.as_deref())
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        drop(proc); // release process lock before entering the event loop

        // Stream Codex events → ACP SessionNotifications.
        let stop_reason = loop {
            let event = match tokio::time::timeout(self.prompt_timeout, event_rx.recv()).await {
                Err(_elapsed) => {
                    warn!(session_id, "codex: prompt timed out");
                    break StopReason::EndTurn;
                }
                Ok(Ok(e)) => e,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                    warn!(session_id, lagged = n, "codex: event channel lagged");
                    continue;
                }
                Ok(Err(_)) => {
                    break StopReason::EndTurn;
                }
            };

            match event {
                CodexEvent::TextDelta { text } => {
                    let notif = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            ContentBlock::from(text),
                        )),
                    );
                    if let Err(e) = nats_client.session_notification(notif).await {
                        warn!(session_id, error = %e, "codex: failed to send text notification");
                    }
                }

                CodexEvent::ToolStarted { id, name, input } => {
                    let tool_call = ToolCall::new(id.clone(), name.clone())
                        .status(ToolCallStatus::InProgress)
                        .raw_input(input)
                        .kind(ToolKind::Execute);
                    let notif = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::ToolCall(tool_call),
                    );
                    if let Err(e) = nats_client.session_notification(notif).await {
                        warn!(session_id, error = %e, "codex: failed to send tool start notification");
                    }
                }

                CodexEvent::ToolCompleted { id, output } => {
                    let update = ToolCallUpdate::new(
                        id,
                        ToolCallUpdateFields::new()
                            .status(ToolCallStatus::Completed)
                            .raw_output(serde_json::Value::String(output)),
                    );
                    let notif = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::ToolCallUpdate(update),
                    );
                    if let Err(e) = nats_client.session_notification(notif).await {
                        warn!(session_id, error = %e, "codex: failed to send tool complete notification");
                    }
                }

                CodexEvent::TurnCompleted => {
                    break StopReason::EndTurn;
                }

                CodexEvent::Error { message } => {
                    warn!(session_id, error = %message, "codex: turn error");
                    break StopReason::EndTurn;
                }
            }
        };

        Ok(PromptResponse::new(stop_reason))
    }

    async fn cancel(
        &self,
        req: CancelNotification,
    ) -> agent_client_protocol::Result<()> {
        let session_id = req.session_id.to_string();

        let thread_id = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_id).map(|s| s.thread_id.clone())
        };

        // Only interrupt if the process is already alive — do not spawn a new
        // one just to cancel a turn that can't exist in a dead process.
        if let Some(thread_id) = thread_id {
            let guard = self.process.lock().await;
            if let Some(p) = guard.as_ref()
                && p.is_alive()
                && let Err(e) = p.turn_interrupt(&thread_id).await
            {
                warn!(session_id, error = %e, "codex: turn_interrupt failed");
            }
        }

        Ok(())
    }
}

// ── Test helpers (same module scope → access to private fields) ───────────────

#[cfg(test)]
impl CodexAgent {
    /// Insert a session directly, bypassing the Codex subprocess.
    async fn test_insert_session(&self, id: &str, cwd: &str, model: Option<String>) {
        self.sessions.lock().await.insert(
            id.to_string(),
            CodexSession {
                thread_id: format!("thread-{id}"),
                cwd: cwd.to_string(),
                model,
            },
        );
    }

    async fn test_session_model(&self, id: &str) -> Option<String> {
        self.sessions.lock().await.get(id).and_then(|s| s.model.clone())
    }

    async fn test_session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    fn test_prompt_timeout(&self) -> Duration {
        self.prompt_timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        Agent, AuthenticateRequest, AuthMethodId, CancelNotification, CloseSessionRequest,
        ForkSessionRequest, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
        PromptRequest, ProtocolVersion, ResumeSessionRequest,
        SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
        SetSessionModelRequest,
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// Spin up a minimal fake NATS server and return a connected client.
    /// The server handles the INFO/CONNECT/PING handshake only; no messages
    /// are published by the session tests so the connection can idle after that.
    async fn fake_nats_client() -> async_nats::Client {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let (reader, mut writer) = stream.into_split();
                writer
                    .write_all(
                        b"INFO {\"server_id\":\"test\",\"version\":\"2.10.0\",\
                          \"max_payload\":1048576,\"proto\":1,\"headers\":true}\r\n",
                    )
                    .await
                    .ok();
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.starts_with("CONNECT") {
                        writer.write_all(b"+OK\r\n").await.ok();
                    } else if line.starts_with("PING") {
                        writer.write_all(b"PONG\r\n").await.ok();
                    }
                }
            }
        });

        async_nats::connect(format!("nats://127.0.0.1:{port}")).await.unwrap()
    }

    async fn make_agent() -> CodexAgent {
        let acp_prefix = AcpPrefix::new("test").unwrap();
        CodexAgent::new(fake_nats_client().await, acp_prefix, "o4-mini")
    }

    // ── close_session ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn close_session_removes_session() {
        let agent = make_agent().await;
        agent.test_insert_session("s1", "/tmp", None).await;
        assert_eq!(agent.test_session_count().await, 1);

        agent.close_session(CloseSessionRequest::new("s1")).await.unwrap();
        assert_eq!(agent.test_session_count().await, 0);
    }

    #[tokio::test]
    async fn close_session_unknown_id_is_noop() {
        let agent = make_agent().await;
        // Must not return an error for unknown session ids.
        agent.close_session(CloseSessionRequest::new("nonexistent")).await.unwrap();
    }

    // ── load_session ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn load_session_returns_state() {
        let agent = make_agent().await;
        agent.test_insert_session("s2", "/home/user", Some("o3".to_string())).await;

        let resp = agent
            .load_session(LoadSessionRequest::new("s2", "/home/user"))
            .await
            .unwrap();
        assert_eq!(resp.models.unwrap().current_model_id.to_string(), "o3");
    }

    #[tokio::test]
    async fn load_session_not_found_returns_error() {
        let agent = make_agent().await;
        assert!(agent.load_session(LoadSessionRequest::new("missing", "/")).await.is_err());
    }

    // ── set_session_model ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_model_updates_model() {
        let agent = make_agent().await;
        agent.test_insert_session("s3", "/tmp", None).await;

        agent
            .set_session_model(SetSessionModelRequest::new("s3", "o3"))
            .await
            .unwrap();
        assert_eq!(agent.test_session_model("s3").await.as_deref(), Some("o3"));
    }

    #[tokio::test]
    async fn set_session_model_rejects_unknown_model() {
        let agent = make_agent().await;
        agent.test_insert_session("s4", "/tmp", None).await;

        let err = agent
            .set_session_model(SetSessionModelRequest::new("s4", "gpt-99"))
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown model"));
    }

    #[tokio::test]
    async fn set_session_model_rejects_unknown_session() {
        let agent = make_agent().await;
        assert!(agent
            .set_session_model(SetSessionModelRequest::new("missing", "o3"))
            .await
            .is_err());
    }

    // ── list_sessions ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_returns_sorted() {
        let agent = make_agent().await;
        agent.test_insert_session("zzz", "/c", None).await;
        agent.test_insert_session("aaa", "/a", None).await;
        agent.test_insert_session("mmm", "/b", None).await;

        let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
        let ids: Vec<_> = resp.sessions.iter().map(|s| s.session_id.to_string()).collect();
        assert_eq!(ids, vec!["aaa", "mmm", "zzz"]);
    }

    #[tokio::test]
    async fn list_sessions_empty() {
        let agent = make_agent().await;
        let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
        assert!(resp.sessions.is_empty());
    }

    // ── default_model validation ──────────────────────────────────────────────

    #[tokio::test]
    async fn default_model_added_when_not_in_list() {
        // CODEX_MODELS does not include "custom-model"
        let nats = fake_nats_client().await;
        let acp_prefix = AcpPrefix::new("test").unwrap();
        // Build agent with a default model not in the env-derived list.
        // Since CODEX_MODELS is not set in test env, the agent uses the default
        // list (o4-mini, o3, gpt-4o); "custom-model" is not among them.
        let agent = CodexAgent::new(nats, acp_prefix, "custom-model");

        // session_model_state should include "custom-model" in available list.
        let state = agent.session_model_state(None);
        let ids: Vec<_> = state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        assert!(ids.contains(&"custom-model".to_string()), "available: {ids:?}");
        assert_eq!(state.current_model_id.to_string(), "custom-model");
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn initialize_returns_latest_protocol_version() {
        let agent = make_agent().await;
        let resp = agent
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await
            .unwrap();
        assert_eq!(resp.protocol_version, ProtocolVersion::LATEST);
    }

    #[tokio::test]
    async fn initialize_advertises_load_session_capability() {
        let agent = make_agent().await;
        let resp = agent
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await
            .unwrap();
        assert!(resp.agent_capabilities.load_session, "load_session should be true");
    }

    #[tokio::test]
    async fn initialize_advertises_session_capabilities() {
        let agent = make_agent().await;
        let resp = agent
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await
            .unwrap();
        let sc = resp.agent_capabilities.session_capabilities;
        assert!(sc.fork.is_some(), "fork capability should be advertised");
        assert!(sc.list.is_some(), "list capability should be advertised");
        assert!(sc.resume.is_some(), "resume capability should be advertised");
        assert!(sc.close.is_some(), "close capability should be advertised");
    }

    #[tokio::test]
    async fn initialize_includes_agent_info() {
        let agent = make_agent().await;
        let resp = agent
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await
            .unwrap();
        let info = resp.agent_info.expect("agent_info should be present");
        assert_eq!(info.name, "trogon-codex-runner");
    }

    // ── authenticate ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn authenticate_always_succeeds() {
        let agent = make_agent().await;
        // Any method_id should succeed — Codex uses env-based auth.
        agent
            .authenticate(AuthenticateRequest::new(AuthMethodId::from("any-method")))
            .await
            .unwrap();
    }

    // ── set_session_mode ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_mode_always_succeeds() {
        let agent = make_agent().await;
        // Codex has no named permission modes — silently accepted.
        agent
            .set_session_mode(SetSessionModeRequest::new("s1", "whatever-mode"))
            .await
            .unwrap();
    }

    // ── set_session_config_option ─────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_config_option_returns_empty_list() {
        let agent = make_agent().await;
        let resp: SetSessionConfigOptionResponse = agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "s1",
                "some-option",
                "some-value",
            ))
            .await
            .unwrap();
        assert!(
            resp.config_options.is_empty(),
            "config_options should be empty: {:?}",
            resp.config_options
        );
    }

    // ── session_mode_state ────────────────────────────────────────────────────

    #[tokio::test]
    async fn session_mode_state_current_is_default() {
        let agent = make_agent().await;
        let state = agent.session_mode_state();
        assert_eq!(state.current_mode_id.to_string(), "default");
    }

    #[tokio::test]
    async fn session_mode_state_lists_one_mode() {
        let agent = make_agent().await;
        let state = agent.session_mode_state();
        assert_eq!(state.available_modes.len(), 1);
        assert_eq!(state.available_modes[0].id.to_string(), "default");
    }

    // ── session_model_state ───────────────────────────────────────────────────

    #[tokio::test]
    async fn session_model_state_uses_agent_default_when_no_override() {
        let agent = make_agent().await;
        let state = agent.session_model_state(None);
        assert_eq!(state.current_model_id.to_string(), "o4-mini");
    }

    #[tokio::test]
    async fn session_model_state_uses_provided_override() {
        let agent = make_agent().await;
        let state = agent.session_model_state(Some("o3"));
        assert_eq!(state.current_model_id.to_string(), "o3");
    }

    #[tokio::test]
    async fn session_model_state_lists_default_models() {
        let agent = make_agent().await;
        let state = agent.session_model_state(None);
        let ids: Vec<_> =
            state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        // Default list is o4-mini, o3, gpt-4o (when CODEX_MODELS env is absent).
        assert!(ids.contains(&"o4-mini".to_string()), "missing o4-mini: {ids:?}");
        assert!(ids.contains(&"o3".to_string()), "missing o3: {ids:?}");
        assert!(ids.contains(&"gpt-4o".to_string()), "missing gpt-4o: {ids:?}");
    }

    // ── resume_session error path ─────────────────────────────────────────────

    #[tokio::test]
    async fn resume_session_returns_error_for_unknown_session() {
        let agent = make_agent().await;
        // Must fail before spawning any subprocess.
        let err = agent
            .resume_session(ResumeSessionRequest::new("nonexistent", "/"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    // ── fork_session error path ───────────────────────────────────────────────

    #[tokio::test]
    async fn fork_session_returns_error_for_unknown_source_session() {
        let agent = make_agent().await;
        let err = agent
            .fork_session(ForkSessionRequest::new("nonexistent", "/fork"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    // ── prompt error path ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_returns_error_for_unknown_session() {
        let agent = make_agent().await;
        // With no content blocks the lookup still fails first — tests error path.
        let err = agent
            .prompt(PromptRequest::new("unknown-session", vec![]))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    // ── cancel noop paths ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_noop_for_unknown_session() {
        let agent = make_agent().await;
        // Should succeed silently — no session to interrupt.
        agent.cancel(CancelNotification::new("no-such-session")).await.unwrap();
    }

    #[tokio::test]
    async fn cancel_noop_when_no_process_running() {
        let agent = make_agent().await;
        agent.test_insert_session("s-cancel", "/tmp", None).await;
        // Process has never been spawned (None) → no interrupt attempted.
        agent.cancel(CancelNotification::new("s-cancel")).await.unwrap();
    }

    // ── CODEX_MODELS env var parsing ──────────────────────────────────────────

    // These tests mutate the process environment so they use a mutex to prevent
    // races when `cargo test` runs them concurrently within the same process.
    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
        std::sync::OnceLock::new();

    fn env_lock() -> &'static std::sync::Mutex<()> {
        ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[tokio::test]
    async fn codex_models_env_var_parsed_correctly() {
        let _guard = env_lock().lock().unwrap();
        // SAFETY: single-threaded section protected by mutex.
        unsafe { std::env::set_var("CODEX_MODELS", "m1:Model One,m2:Model Two") };
        let nats = fake_nats_client().await;
        let agent = CodexAgent::new(nats, AcpPrefix::new("test").unwrap(), "m1");
        unsafe { std::env::remove_var("CODEX_MODELS") };

        let state = agent.session_model_state(None);
        let ids: Vec<_> =
            state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        assert_eq!(ids, vec!["m1", "m2"], "models: {ids:?}");
    }

    #[tokio::test]
    async fn codex_models_all_malformed_falls_back_to_defaults() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("CODEX_MODELS", "bad,also-bad,no-colon") };
        let nats = fake_nats_client().await;
        let agent = CodexAgent::new(nats, AcpPrefix::new("test").unwrap(), "o4-mini");
        unsafe { std::env::remove_var("CODEX_MODELS") };

        let state = agent.session_model_state(None);
        let ids: Vec<_> =
            state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        // Falls back to hardcoded defaults when all entries are malformed.
        assert!(ids.contains(&"o4-mini".to_string()), "expected defaults: {ids:?}");
        assert!(ids.contains(&"o3".to_string()), "expected defaults: {ids:?}");
    }

    #[tokio::test]
    async fn codex_models_partially_malformed_skips_bad_entries() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("CODEX_MODELS", "good:Good Model,bad-entry,another:Another") };
        let nats = fake_nats_client().await;
        let agent = CodexAgent::new(nats, AcpPrefix::new("test").unwrap(), "good");
        unsafe { std::env::remove_var("CODEX_MODELS") };

        let state = agent.session_model_state(None);
        let ids: Vec<_> =
            state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        // "bad-entry" (no colon) is skipped; the two valid ones are kept.
        assert!(ids.contains(&"good".to_string()), "models: {ids:?}");
        assert!(ids.contains(&"another".to_string()), "models: {ids:?}");
        assert!(!ids.contains(&"bad-entry".to_string()), "bad-entry should be skipped: {ids:?}");
    }

    // ── CODEX_PROMPT_TIMEOUT_SECS invalid value ───────────────────────────────

    #[tokio::test]
    async fn prompt_timeout_invalid_env_var_falls_back_to_default() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("CODEX_PROMPT_TIMEOUT_SECS", "not_a_number") };
        let nats = fake_nats_client().await;
        let agent = CodexAgent::new(nats, AcpPrefix::new("test").unwrap(), "o4-mini");
        unsafe { std::env::remove_var("CODEX_PROMPT_TIMEOUT_SECS") };
        // Default is 7200 s. Verify prompt_timeout was set (observable via
        // the fact that construction didn't panic and the agent works normally).
        assert_eq!(agent.test_prompt_timeout(), std::time::Duration::from_secs(7200));
    }

    // ── load_session returns agent default when session has no model override ──

    #[tokio::test]
    async fn load_session_returns_agent_default_when_no_per_session_model() {
        let agent = make_agent().await;
        agent.test_insert_session("s-load", "/tmp", None).await;

        let resp = agent
            .load_session(LoadSessionRequest::new("s-load", "/tmp"))
            .await
            .unwrap();
        let model_state = resp.models.expect("models present");
        assert_eq!(
            model_state.current_model_id.to_string(),
            "o4-mini",
            "should fall back to agent default"
        );
    }
}
