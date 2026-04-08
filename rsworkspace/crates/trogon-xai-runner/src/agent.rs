use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::Client as _;
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
    StopReason,
};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::client::{InputItem, Message, XaiClient, XaiEvent};
use crate::http_client::XaiHttpClient;

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

/// Per-session state held in memory.
///
/// Unlike the Codex runner (which delegates state to the subprocess), xAI is a
/// stateless HTTP API. The runner must maintain conversation history locally and
/// replay it on every turn — unless `last_response_id` is set, in which case
/// the xAI server already holds the prior context and only the new message is sent.
struct XaiSession {
    cwd: String,
    /// Per-session model override. None means use the agent default.
    model: Option<String>,
    /// API key bound to this session at `new_session` time.
    /// Falls back to the agent-wide `global_api_key` if None.
    api_key: Option<String>,
    /// Conversation history (user + assistant turns).
    /// Trimmed to `max_history` entries when it grows too large.
    history: Vec<Message>,
    /// The `id` from the last xAI response.
    ///
    /// When set, the next turn sends only the new user message and passes this
    /// as `previous_response_id` — the server has the prior context. When None
    /// (new session, fork, or after a stream error), the full history is sent.
    last_response_id: Option<String>,
}

/// ACP Agent implementation backed by xAI's Grok API (Responses API).
///
/// Each `XaiAgent` manages multiple in-memory sessions. Because xAI exposes a
/// stateless HTTP endpoint, the runner maintains conversation history locally
/// and builds the full input on each turn (or uses `previous_response_id` as a
/// shortcut when the server still holds the prior response in its cache).
///
/// This mirrors the structure of `trogon-codex-runner` but replaces the
/// subprocess (`codex app-server`) with an HTTP client — making the core logic
/// WASM-friendly since it has no OS process or signal dependencies.
///
/// `H` abstracts the xAI HTTP client (default: `XaiClient`) so tests can inject
/// a mock without spinning up a TCP server.
pub struct XaiAgent<H = XaiClient> {
    nats: async_nats::Client,
    acp_prefix: AcpPrefix,
    client: Arc<H>,
    sessions: Arc<Mutex<HashMap<String, XaiSession>>>,
    /// In-flight cancel channels, one per active prompt. Sending `()` stops the
    /// streaming loop and causes `prompt()` to return early.
    cancel_senders: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    default_model: String,
    /// Per-chunk inactivity timeout. Fires if no SSE chunk arrives within this
    /// window — a slow but continuously streaming response is NOT cut off.
    prompt_timeout: Duration,
    available_models: Vec<ModelInfo>,
    /// Agent-wide API key from `XAI_API_KEY` env var. Used as the fallback when
    /// a session has no per-session key.
    global_api_key: Option<String>,
    /// Key captured from the last `authenticate()` call, consumed by the next
    /// `new_session()`. Race-safe only when clients authenticate before creating
    /// a session (the typical ACP flow).
    pending_api_key: Arc<Mutex<Option<String>>>,
    /// Optional system prompt prepended to every conversation.
    system_prompt: Option<String>,
    /// Maximum number of history messages kept per session.
    /// Oldest messages are dropped individually when the limit is exceeded.
    max_history: usize,
    /// Maximum agentic tool-call turns per prompt (passed to the Responses API).
    max_turns: Option<u32>,
}

impl XaiAgent<XaiClient> {
    /// Create a new `XaiAgent` backed by the real xAI HTTP API.
    ///
    /// Environment variables read at construction:
    /// - `XAI_PROMPT_TIMEOUT_SECS` — per-chunk timeout (default: 300; 0 = default)
    /// - `XAI_MAX_HISTORY_MESSAGES` — max history entries (default: 20; 0 = default)
    /// - `XAI_MODELS` — comma-separated `id:label` pairs
    /// - `XAI_BASE_URL` — override the xAI API base URL
    /// - `XAI_SYSTEM_PROMPT` — optional system prompt
    /// - `XAI_MAX_TURNS` — max tool-call turns (default: 10; 0 = server default)
    pub fn new(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self::with_client(nats, acp_prefix, default_model, api_key, XaiClient::new())
    }
}

impl<H: XaiHttpClient> XaiAgent<H> {
    /// Create an `XaiAgent` with a custom HTTP client. Used in tests.
    pub fn with_client(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        client: H,
    ) -> Self {
        let default_model: String = default_model.into();
        let api_key_str: String = api_key.into();
        let global_api_key = if api_key_str.is_empty() { None } else { Some(api_key_str) };

        let prompt_timeout = std::env::var("XAI_PROMPT_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(300));

        let mut available_models = std::env::var("XAI_MODELS")
            .ok()
            .map(|s| {
                s.split(',')
                    .filter_map(|entry| match entry.split_once(':') {
                        Some((id, label)) => Some(ModelInfo::new(
                            id.trim().to_string(),
                            label.trim().to_string(),
                        )),
                        None => {
                            warn!(
                                entry,
                                "XAI_MODELS: skipping malformed entry (expected 'id:label')"
                            );
                            None
                        }
                    })
                    .collect()
            })
            .filter(|v: &Vec<ModelInfo>| !v.is_empty())
            .unwrap_or_else(|| {
                vec![
                    ModelInfo::new("grok-3", "Grok 3"),
                    ModelInfo::new("grok-3-mini", "Grok 3 Mini"),
                ]
            });

        if !available_models
            .iter()
            .any(|m| m.model_id.0.as_ref() == default_model.as_str())
        {
            warn!(model = %default_model, "default model not in available list; adding it");
            available_models.push(ModelInfo::new(default_model.clone(), default_model.clone()));
        }

        let system_prompt = std::env::var("XAI_SYSTEM_PROMPT")
            .ok()
            .filter(|s| !s.is_empty());

        let max_history = std::env::var("XAI_MAX_HISTORY_MESSAGES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(20);

        let max_turns = std::env::var("XAI_MAX_TURNS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&n| n > 0)
            .or(Some(10));

        Self {
            nats,
            acp_prefix,
            client: Arc::new(client),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            cancel_senders: Arc::new(Mutex::new(HashMap::new())),
            default_model,
            prompt_timeout,
            available_models,
            global_api_key,
            pending_api_key: Arc::new(Mutex::new(None)),
            system_prompt,
            max_history,
            max_turns,
        }
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
        SessionModeState::new(
            "default".to_string(),
            vec![SessionMode::new("default", "Default")],
        )
    }

    fn session_model_state(&self, current: Option<&str>) -> SessionModelState {
        let current = current.unwrap_or(&self.default_model).to_string();
        SessionModelState::new(current, self.available_models.clone())
    }

    /// Build the full input array from session history + the new user message.
    fn build_input(&self, history: &[Message], user_input: &str) -> Vec<InputItem> {
        let mut items = Vec::with_capacity(history.len() + 2);
        if let Some(sp) = &self.system_prompt {
            items.push(InputItem::system(sp));
        }
        for msg in history {
            if msg.role == "assistant" {
                items.push(InputItem::assistant(msg.content_str()));
            } else {
                items.push(InputItem::user(msg.content_str()));
            }
        }
        items.push(InputItem::user(user_input));
        items
    }
}

#[async_trait(?Send)]
impl<H: XaiHttpClient + 'static> agent_client_protocol::Agent for XaiAgent<H> {
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
                "trogon-xai-runner",
                env!("CARGO_PKG_VERSION"),
            )))
    }

    async fn authenticate(
        &self,
        req: AuthenticateRequest,
    ) -> agent_client_protocol::Result<AuthenticateResponse> {
        // Extract XAI_API_KEY from the request meta if provided by the client.
        // This allows per-user API keys in multi-tenant deployments.
        if let Some(key) = req
            .meta
            .as_ref()
            .and_then(|m| m.get("XAI_API_KEY"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        {
            info!("xai: client authenticated with user-provided API key");
            *self.pending_api_key.lock().await = Some(key);
        }
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(
        &self,
        req: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        let cwd = req.cwd.to_string_lossy().into_owned();
        let session_id = Uuid::new_v4().to_string();

        // Consume any pending API key from a preceding authenticate() call;
        // fall back to the agent-wide key.
        let api_key = self.pending_api_key.lock().await.take()
            .or_else(|| self.global_api_key.clone());

        self.sessions.lock().await.insert(
            session_id.clone(),
            XaiSession {
                cwd,
                model: None,
                api_key,
                history: Vec::new(),
                last_response_id: None,
            },
        );

        info!(session_id, "xai: new session");
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
        match sessions.get(&session_id) {
            Some(s) => Ok(LoadSessionResponse::new()
                .modes(self.session_mode_state())
                .models(self.session_model_state(s.model.as_deref()))),
            None => Err(internal_error(format!("session {session_id} not found"))),
        }
    }

    async fn resume_session(
        &self,
        req: ResumeSessionRequest,
    ) -> agent_client_protocol::Result<ResumeSessionResponse> {
        let session_id = req.session_id.to_string();
        if !self.sessions.lock().await.contains_key(&session_id) {
            return Err(internal_error(format!("session {session_id} not found")));
        }
        Ok(ResumeSessionResponse::new())
    }

    async fn fork_session(
        &self,
        req: ForkSessionRequest,
    ) -> agent_client_protocol::Result<ForkSessionResponse> {
        let source_id = req.session_id.to_string();
        let cwd = req.cwd.to_string_lossy().into_owned();

        let (inherited_model, inherited_key, history) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&source_id)
                .ok_or_else(|| internal_error(format!("session {source_id} not found")))?;
            (s.model.clone(), s.api_key.clone(), s.history.clone())
        };

        let new_session_id = Uuid::new_v4().to_string();
        self.sessions.lock().await.insert(
            new_session_id.clone(),
            XaiSession {
                cwd,
                model: inherited_model.clone(),
                api_key: inherited_key,
                history,
                // Forks start without a response ID — xAI's server cache is per-response,
                // so the fork must replay its own history on the first turn.
                last_response_id: None,
            },
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

        // Cancel any in-flight prompt before removing the session.
        let sender = self.cancel_senders.lock().await.remove(&session_id);
        if let Some(tx) = sender {
            let _ = tx.send(());
        }

        self.sessions.lock().await.remove(&session_id);
        info!(session_id, "xai: session closed");
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
        // xAI has no ACP permission modes — silently accept.
        Ok(SetSessionModeResponse::new())
    }

    async fn set_session_model(
        &self,
        req: SetSessionModelRequest,
    ) -> agent_client_protocol::Result<SetSessionModelResponse> {
        let session_id = req.session_id.to_string();
        let model_id = req.model_id.to_string();

        if !self
            .available_models
            .iter()
            .any(|m| m.model_id.0.as_ref() == model_id)
        {
            return Err(internal_error(format!("unknown model: {model_id}")));
        }

        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(&session_id) {
            Some(s) => {
                s.model = Some(model_id.clone());
                info!(session_id, model = %model_id, "xai: set_session_model");
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

    async fn prompt(&self, req: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        let session_id = req.session_id.to_string();
        let nats_client = self.make_nats_client(&req.session_id)?;

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
            warn!(session_id, "xai: prompt has no text blocks");
        }

        // Snapshot session state — release lock before streaming.
        let (model, api_key, history, last_response_id) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&session_id)
                .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
            (
                s.model.clone(),
                s.api_key.clone(),
                s.history.clone(),
                s.last_response_id.clone(),
            )
        };

        let model = model.as_deref().unwrap_or(&self.default_model);
        let api_key = api_key
            .or_else(|| self.global_api_key.clone())
            .ok_or_else(|| {
                internal_error("no API key for session — set XAI_API_KEY or authenticate first")
            })?;

        // When the server still holds the prior response, send only the new
        // user message. Otherwise replay the full history.
        let (input, prev_response_id) = match &last_response_id {
            Some(id) => (vec![InputItem::user(&user_input)], Some(id.as_str())),
            None => (self.build_input(&history, &user_input), None),
        };

        // Register a cancel channel so cancel() can abort this prompt.
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        self.cancel_senders
            .lock()
            .await
            .insert(session_id.clone(), cancel_tx);

        let mut stream = self
            .client
            .chat_stream(model, &input, &api_key, &[], prev_response_id, self.max_turns)
            .await;

        let mut assistant_text = String::new();
        let mut new_response_id: Option<String> = None;
        let mut pending_cancel = true;

        let stop_reason = loop {
            tokio::select! {
                biased;
                result = &mut cancel_rx, if pending_cancel => {
                    pending_cancel = false;
                    if result.is_ok() {
                        info!(session_id, "xai: prompt cancelled");
                        break StopReason::EndTurn;
                    }
                    // Sender dropped without sending — not a real cancel; continue.
                }
                event = tokio::time::timeout(self.prompt_timeout, stream.next()) => {
                    match event {
                        Err(_elapsed) => {
                            warn!(session_id, "xai: prompt timed out");
                            break StopReason::EndTurn;
                        }
                        Ok(None) => break StopReason::EndTurn,
                        Ok(Some(ev)) => match ev {
                            XaiEvent::TextDelta { text } => {
                                assistant_text.push_str(&text);
                                let notif = SessionNotification::new(
                                    session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::from(text),
                                    )),
                                );
                                if let Err(e) = nats_client.session_notification(notif).await {
                                    warn!(session_id, error = %e, "xai: failed to send text notification");
                                }
                            }
                            XaiEvent::ResponseId { id } => {
                                new_response_id = Some(id);
                            }
                            XaiEvent::Finished { .. } | XaiEvent::Done => {
                                break StopReason::EndTurn;
                            }
                            XaiEvent::Error { message } => {
                                warn!(session_id, error = %message, "xai: stream error");
                                break StopReason::EndTurn;
                            }
                            // FunctionCall and ServerToolCompleted are not forwarded
                            // in this implementation — server-side tools run silently
                            // and their output arrives as text deltas.
                            XaiEvent::FunctionCall { .. }
                            | XaiEvent::ServerToolCompleted { .. }
                            | XaiEvent::Usage { .. } => {}
                        },
                    }
                }
            }
        };

        self.cancel_senders.lock().await.remove(&session_id);

        // Update session history with this turn's exchange.
        {
            let mut sessions = self.sessions.lock().await;
            if let Some(s) = sessions.get_mut(&session_id) {
                s.history.push(Message::user(user_input));
                if !assistant_text.is_empty() {
                    s.history.push(Message::assistant_text(assistant_text));
                }
                // Trim oldest messages when history exceeds the limit.
                while s.history.len() > self.max_history {
                    s.history.remove(0);
                }
                // Only update the response ID if we received one; keep the old
                // ID on stream errors so the next turn can still use it.
                if new_response_id.is_some() {
                    s.last_response_id = new_response_id;
                }
            }
            // If session was closed during streaming, silently discard history update.
        }

        Ok(PromptResponse::new(stop_reason))
    }

    async fn cancel(&self, req: CancelNotification) -> agent_client_protocol::Result<()> {
        let session_id = req.session_id.to_string();
        let sender = self.cancel_senders.lock().await.remove(&session_id);
        if let Some(tx) = sender {
            let _ = tx.send(());
        }
        Ok(())
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

#[cfg(test)]
impl<H: XaiHttpClient> XaiAgent<H> {
    async fn test_insert_session(&self, id: &str, cwd: &str, model: Option<String>) {
        self.sessions.lock().await.insert(
            id.to_string(),
            XaiSession {
                cwd: cwd.to_string(),
                model,
                api_key: Some("test-key".to_string()),
                history: Vec::new(),
                last_response_id: None,
            },
        );
    }

    async fn test_session_model(&self, id: &str) -> Option<String> {
        self.sessions.lock().await.get(id).and_then(|s| s.model.clone())
    }

    async fn test_session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    async fn test_history_len(&self, id: &str) -> usize {
        self.sessions
            .lock()
            .await
            .get(id)
            .map(|s| s.history.len())
            .unwrap_or(0)
    }

    fn test_prompt_timeout(&self) -> Duration {
        self.prompt_timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        Agent, AuthMethodId, AuthenticateRequest, CancelNotification, CloseSessionRequest,
        ForkSessionRequest, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
        PromptRequest, ProtocolVersion, ResumeSessionRequest, SetSessionConfigOptionRequest,
        SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModelRequest,
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    use crate::client::XaiEvent;
    use crate::http_client::mock::MockXaiHttpClient;

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

        async_nats::connect(format!("nats://127.0.0.1:{port}"))
            .await
            .unwrap()
    }

    type TestAgent = XaiAgent<std::sync::Arc<MockXaiHttpClient>>;

    fn make_mock_agent(nats: async_nats::Client) -> TestAgent {
        let mock = std::sync::Arc::new(MockXaiHttpClient::new());
        let acp_prefix = AcpPrefix::new("test").unwrap();
        XaiAgent::with_client(nats, acp_prefix, "grok-3", "test-key", mock)
    }

    async fn make_agent() -> TestAgent {
        make_mock_agent(fake_nats_client().await)
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
        agent.close_session(CloseSessionRequest::new("nonexistent")).await.unwrap();
    }

    // ── load_session ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn load_session_returns_state() {
        let agent = make_agent().await;
        agent.test_insert_session("s2", "/home/user", Some("grok-3-mini".to_string())).await;
        let resp = agent
            .load_session(LoadSessionRequest::new("s2", "/home/user"))
            .await
            .unwrap();
        assert_eq!(resp.models.unwrap().current_model_id.to_string(), "grok-3-mini");
    }

    #[tokio::test]
    async fn load_session_not_found_returns_error() {
        let agent = make_agent().await;
        assert!(agent.load_session(LoadSessionRequest::new("missing", "/")).await.is_err());
    }

    // ── resume_session ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resume_session_returns_error_for_unknown_session() {
        let agent = make_agent().await;
        let err = agent
            .resume_session(ResumeSessionRequest::new("nonexistent", "/"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    // ── fork_session ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fork_session_inherits_model() {
        let agent = make_agent().await;
        agent.test_insert_session("src", "/tmp", Some("grok-3-mini".to_string())).await;
        let resp = agent
            .fork_session(ForkSessionRequest::new("src", "/fork"))
            .await
            .unwrap();
        let new_id = resp.session_id.to_string();
        assert_eq!(agent.test_session_model(&new_id).await.as_deref(), Some("grok-3-mini"));
    }

    #[tokio::test]
    async fn fork_session_returns_error_for_unknown_source() {
        let agent = make_agent().await;
        let err = agent
            .fork_session(ForkSessionRequest::new("nonexistent", "/fork"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    // ── set_session_model ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_model_updates_model() {
        let agent = make_agent().await;
        agent.test_insert_session("s3", "/tmp", None).await;
        agent.set_session_model(SetSessionModelRequest::new("s3", "grok-3-mini")).await.unwrap();
        assert_eq!(agent.test_session_model("s3").await.as_deref(), Some("grok-3-mini"));
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

    // ── initialize ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn initialize_returns_latest_protocol_version() {
        let agent = make_agent().await;
        let resp = agent.initialize(InitializeRequest::new(ProtocolVersion::LATEST)).await.unwrap();
        assert_eq!(resp.protocol_version, ProtocolVersion::LATEST);
    }

    #[tokio::test]
    async fn initialize_advertises_session_capabilities() {
        let agent = make_agent().await;
        let resp = agent.initialize(InitializeRequest::new(ProtocolVersion::LATEST)).await.unwrap();
        let sc = resp.agent_capabilities.session_capabilities;
        assert!(sc.fork.is_some());
        assert!(sc.list.is_some());
        assert!(sc.resume.is_some());
        assert!(sc.close.is_some());
    }

    #[tokio::test]
    async fn initialize_includes_agent_info() {
        let agent = make_agent().await;
        let resp = agent.initialize(InitializeRequest::new(ProtocolVersion::LATEST)).await.unwrap();
        let info = resp.agent_info.expect("agent_info should be present");
        assert_eq!(info.name, "trogon-xai-runner");
    }

    // ── authenticate ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn authenticate_always_succeeds() {
        let agent = make_agent().await;
        agent
            .authenticate(AuthenticateRequest::new(AuthMethodId::from("any-method")))
            .await
            .unwrap();
    }

    // ── set_session_mode ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_mode_always_succeeds() {
        let agent = make_agent().await;
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
        assert!(resp.config_options.is_empty());
    }

    // ── prompt ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_returns_error_for_unknown_session() {
        let agent = make_agent().await;
        let err = agent
            .prompt(PromptRequest::new("unknown-session", vec![]))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    #[tokio::test]
    async fn prompt_returns_error_for_invalid_session_id() {
        let agent = make_agent().await;
        let err = agent
            .prompt(PromptRequest::new("invalid.session.id", vec![]))
            .await
            .unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[tokio::test]
    async fn prompt_appends_history_after_turn() {
        let agent = make_agent().await;
        agent.test_insert_session("h1", "/tmp", None).await;

        // Queue a response: text delta + done
        agent.client.push_response(vec![
            XaiEvent::TextDelta { text: "hello".to_string() },
            XaiEvent::Done,
        ]);

        agent
            .prompt(PromptRequest::new("h1", vec![ContentBlock::from("hi".to_string())]))
            .await
            .unwrap();

        // user + assistant = 2 history entries
        assert_eq!(agent.test_history_len("h1").await, 2);
    }

    // ── cancel ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_noop_for_unknown_session() {
        let agent = make_agent().await;
        agent.cancel(CancelNotification::new("no-such-session")).await.unwrap();
    }

    // ── model list ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn default_model_added_when_not_in_list() {
        let nats = fake_nats_client().await;
        let mock = std::sync::Arc::new(MockXaiHttpClient::new());
        let acp_prefix = AcpPrefix::new("test").unwrap();
        let agent = XaiAgent::with_client(nats, acp_prefix, "custom-model", "key", mock);
        let state = agent.session_model_state(None);
        let ids: Vec<_> = state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        assert!(ids.contains(&"custom-model".to_string()), "available: {ids:?}");
        assert_eq!(state.current_model_id.to_string(), "custom-model");
    }

    // ── session_mode_state ────────────────────────────────────────────────────

    #[tokio::test]
    async fn session_mode_state_current_is_default() {
        let agent = make_agent().await;
        let state = agent.session_mode_state();
        assert_eq!(state.current_mode_id.to_string(), "default");
    }

    // ── prompt timeout env var ────────────────────────────────────────────────

    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    fn env_lock() -> &'static std::sync::Mutex<()> {
        ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[tokio::test]
    async fn prompt_timeout_invalid_env_var_falls_back_to_default() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", "not_a_number") };
        let nats = fake_nats_client().await;
        let mock = std::sync::Arc::new(MockXaiHttpClient::new());
        let agent =
            XaiAgent::with_client(nats, AcpPrefix::new("test").unwrap(), "grok-3", "key", mock);
        unsafe { std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS") };
        assert_eq!(agent.test_prompt_timeout(), Duration::from_secs(300));
    }
}
