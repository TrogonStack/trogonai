use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol::{
    AgentCapabilities, AuthEnvVar, AuthMethod, AuthMethodAgent, AuthMethodEnvVar,
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ContentChunk, EmbeddedResourceResource, Error, ErrorCode,
    ForkSessionRequest, ForkSessionResponse, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, ModelInfo,
    NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
    ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities,
    SessionCloseCapabilities, SessionConfigOption, SessionConfigOptionValue,
    SessionConfigSelectOption, SessionForkCapabilities, SessionId, SessionInfo,
    SessionListCapabilities, SessionMode, SessionModeState, SessionModelState,
    SessionNotification, SessionResumeCapabilities, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, StopReason, ToolCall,
    ToolCallStatus, ToolKind, UsageUpdate,
};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::client::{FinishReason, InputItem, Message, XaiClient, XaiEvent};
use crate::http_client::XaiHttpClient;
use crate::session_notifier::{NatsSessionNotifier, SessionNotifier};

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

fn invalid_params(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidParams.into(), msg.into())
}

/// xAI server-side tools that can be toggled per session.
///
/// Each entry is `(tool_id, display_label)`. The `tool_id` is sent verbatim in
/// the Responses API `tools` array; the label is shown in the ACP UI.
const AVAILABLE_TOOLS: &[(&str, &str)] = &[
    ("web_search", "Web Search"),
    ("x_search", "X Search"),
];

/// Maximum number of sessions held in memory simultaneously.
///
/// When a new session would exceed this limit, the oldest session (by
/// `created_at`) is evicted with a warning. This prevents unbounded growth in
/// long-running deployments where clients never call `close_session`.
const MAX_SESSIONS: usize = 100;

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
    /// Server-side tool IDs that are currently enabled for this session.
    /// Sent verbatim in the Responses API `tools` array.
    enabled_tools: Vec<String>,
    /// Optional system prompt prepended to every conversation.
    /// Copied from the agent-wide `system_prompt` at session creation time.
    system_prompt: Option<String>,
    /// Wall-clock time at which this session was created. Used for LRU eviction
    /// when the session count reaches `MAX_SESSIONS`.
    created_at: Instant,
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
/// Every external dependency is abstracted behind a trait:
/// - `H: XaiHttpClient` — the xAI API HTTP client
/// - `N: SessionNotifier` — the ACP session notification channel
///
/// Production uses `XaiAgent<XaiClient, NatsSessionNotifier>` (the defaults).
/// Tests inject `MockXaiHttpClient` and `MockSessionNotifier` without any network.
pub struct XaiAgent<H = XaiClient, N = NatsSessionNotifier> {
    notifier: Arc<N>,
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
    /// Oldest messages are dropped in pairs to preserve user/assistant ordering.
    max_history: usize,
    /// Maximum agentic tool-call turns per prompt (passed to the Responses API).
    max_turns: Option<u32>,
}

impl XaiAgent<XaiClient, NatsSessionNotifier> {
    /// Create a new `XaiAgent` backed by the real xAI HTTP API and NATS notifications.
    ///
    /// Environment variables read at construction:
    /// - `XAI_PROMPT_TIMEOUT_SECS` — per-chunk timeout (default: 300; 0 = default)
    /// - `XAI_MAX_HISTORY_MESSAGES` — max history entries (default: 20; 0 = default)
    /// - `XAI_MODELS` — comma-separated `id:label` pairs
    /// - `XAI_BASE_URL` — override the xAI API base URL
    /// - `XAI_SYSTEM_PROMPT` — optional system prompt
    /// - `XAI_MAX_TURNS` — max tool-call turns (default: 10; 0 = server default)
    pub fn new(
        notifier: NatsSessionNotifier,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self::with_deps(notifier, default_model, api_key, XaiClient::new())
    }
}

impl<H: XaiHttpClient, N: SessionNotifier> XaiAgent<H, N> {
    /// Create an `XaiAgent` with explicit dependencies. Used in tests to inject mocks.
    pub fn with_deps(
        notifier: N,
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
                    ModelInfo::new("grok-4", "Grok 4"),
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
            notifier: Arc::new(notifier),
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

    /// Build a `SessionConfigOption` for a single server-side tool toggle.
    fn tool_config_option(tool_id: &str, label: &str, enabled: bool) -> SessionConfigOption {
        SessionConfigOption::select(
            tool_id.to_string(),
            label.to_string(),
            if enabled { "on" } else { "off" }.to_string(),
            vec![
                SessionConfigSelectOption::new("off", "Off"),
                SessionConfigSelectOption::new("on", "On"),
            ],
        )
    }

    /// Evict the oldest session if the map is at capacity.
    ///
    /// Called before inserting a new session so the map never exceeds
    /// `MAX_SESSIONS`. The evicted session is logged as a warning.
    fn maybe_evict_oldest(sessions: &mut HashMap<String, XaiSession>) {
        if sessions.len() < MAX_SESSIONS {
            return;
        }
        if let Some(oldest_id) = sessions
            .iter()
            .min_by_key(|(_, s)| s.created_at)
            .map(|(id, _)| id.clone())
        {
            warn!(session_id = %oldest_id, max = MAX_SESSIONS,
                  "xai: session limit reached — evicting oldest session");
            sessions.remove(&oldest_id);
        }
    }

    /// Build `SessionConfigOption`s for all known server-side tools.
    fn all_tool_config_options(enabled_tools: &[String]) -> Vec<SessionConfigOption> {
        AVAILABLE_TOOLS
            .iter()
            .map(|(id, label)| {
                let enabled = enabled_tools.iter().any(|t| t.as_str() == *id);
                Self::tool_config_option(id, label, enabled)
            })
            .collect()
    }
}

#[async_trait(?Send)]
impl<H: XaiHttpClient + 'static, N: SessionNotifier + 'static> agent_client_protocol::Agent
    for XaiAgent<H, N>
{
    async fn initialize(
        &self,
        _req: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        let mut auth_methods = vec![AuthMethod::EnvVar(
            AuthMethodEnvVar::new(
                "xai-api-key",
                "xAI API Key",
                vec![AuthEnvVar::new("XAI_API_KEY").label("xAI API Key")],
            )
            .link("https://x.ai/api")
            .description("Your personal xAI API key"),
        )];
        if self.global_api_key.is_some() {
            auth_methods.push(AuthMethod::Agent(
                AuthMethodAgent::new("agent", "Use server key")
                    .description("Use the API key configured on this server"),
            ));
        }

        Ok(InitializeResponse::new(ProtocolVersion::LATEST)
            .auth_methods(auth_methods)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(PromptCapabilities::new().embedded_context(true))
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
        // Any method_id is accepted (lenient — clients may use legacy ids like "api-key").
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
        let api_key = self
            .pending_api_key
            .lock()
            .await
            .take()
            .or_else(|| self.global_api_key.clone());

        let mut sessions = self.sessions.lock().await;
        Self::maybe_evict_oldest(&mut sessions);
        sessions.insert(
            session_id.clone(),
            XaiSession {
                cwd,
                model: None,
                api_key,
                history: Vec::new(),
                last_response_id: None,
                enabled_tools: Vec::new(),
                system_prompt: self.system_prompt.clone(),
                created_at: Instant::now(),
            },
        );
        drop(sessions);

        info!(session_id, "xai: new session");
        Ok(NewSessionResponse::new(SessionId::from(session_id))
            .modes(self.session_mode_state())
            .models(self.session_model_state(None))
            .config_options(Self::all_tool_config_options(&[])))
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
                .models(self.session_model_state(s.model.as_deref()))
                .config_options(Self::all_tool_config_options(&s.enabled_tools))),
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

        let (inherited_model, inherited_key, history, inherited_tools, inherited_system_prompt) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&source_id)
                .ok_or_else(|| internal_error(format!("session {source_id} not found")))?;
            (
                s.model.clone(),
                s.api_key.clone(),
                s.history.clone(),
                s.enabled_tools.clone(),
                s.system_prompt.clone(),
            )
        };

        let new_session_id = Uuid::new_v4().to_string();
        let mut sessions = self.sessions.lock().await;
        Self::maybe_evict_oldest(&mut sessions);
        sessions.insert(
            new_session_id.clone(),
            XaiSession {
                cwd,
                model: inherited_model.clone(),
                api_key: inherited_key,
                history,
                // Forks start without a response ID — xAI's server cache is per-response,
                // so the fork must replay its own history on the first turn.
                last_response_id: None,
                enabled_tools: inherited_tools.clone(),
                system_prompt: inherited_system_prompt,
                created_at: Instant::now(),
            },
        );
        drop(sessions);

        Ok(ForkSessionResponse::new(new_session_id)
            .modes(self.session_mode_state())
            .models(self.session_model_state(inherited_model.as_deref()))
            .config_options(Self::all_tool_config_options(&inherited_tools)))
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
        req: SetSessionModeRequest,
    ) -> agent_client_protocol::Result<SetSessionModeResponse> {
        let mode_id = req.mode_id.to_string();
        if mode_id != "default" {
            return Err(invalid_params(format!("unknown mode: {mode_id}")));
        }
        // xAI has no ACP permission modes — silently accept "default".
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
                // Response IDs are model-specific — a stale ID from the previous model
                // would trigger an unnecessary retry on the next prompt. Clear it proactively.
                s.last_response_id = None;
                info!(session_id, model = %model_id, "xai: set_session_model");
                Ok(SetSessionModelResponse::new())
            }
            None => Err(internal_error(format!("session {session_id} not found"))),
        }
    }

    async fn set_session_config_option(
        &self,
        req: SetSessionConfigOptionRequest,
    ) -> agent_client_protocol::Result<SetSessionConfigOptionResponse> {
        let config_id = req.config_id.to_string();
        let session_id = req.session_id.to_string();

        // If this is a known server-side tool toggle and the session exists, apply it.
        let is_known_tool = AVAILABLE_TOOLS.iter().any(|(id, _)| *id == config_id.as_str());
        if is_known_tool {
            if let SessionConfigOptionValue::ValueId { value } = &req.value {
                let val = value.to_string();
                let mut sessions = self.sessions.lock().await;
                if let Some(s) = sessions.get_mut(&session_id) {
                    if val == "on" {
                        if !s.enabled_tools.iter().any(|t| t == &config_id) {
                            s.enabled_tools.push(config_id.clone());
                        }
                    } else {
                        s.enabled_tools.retain(|t| t != &config_id);
                    }
                    info!(session_id, tool = %config_id, enabled = (val == "on"), "xai: tool toggled");
                }
            }
        } else {
            warn!(config_id = %config_id, "xai: set_session_config_option called for unknown option — ignored");
        }

        Ok(SetSessionConfigOptionResponse::new(vec![]))
    }

    async fn prompt(&self, req: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        let session_id = req.session_id.to_string();

        let user_input: String = req
            .prompt
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(t) => Some(t.text.clone()),
                // ACP spec: all agents MUST support ResourceLink. Include the
                // reference as text so the model has full context.
                ContentBlock::ResourceLink(r) => {
                    Some(format!("[Resource: {} | {}]", r.name, r.uri))
                }
                // Embedded resource: include text content directly; note binary
                // blobs as they cannot be forwarded to a text-only API.
                ContentBlock::Resource(r) => match &r.resource {
                    EmbeddedResourceResource::TextResourceContents(t) => Some(t.text.clone()),
                    EmbeddedResourceResource::BlobResourceContents(b) => {
                        let mime = b.mime_type.as_deref().unwrap_or("binary");
                        Some(format!("[Binary resource: {} ({})]", b.uri, mime))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        if user_input.is_empty() {
            warn!(session_id, "xai: prompt contains no text or resource blocks");
        }

        // Snapshot session state — release lock before streaming.
        let (model, api_key, history, last_response_id, enabled_tools) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&session_id)
                .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
            (
                s.model.clone(),
                s.api_key.clone(),
                s.history.clone(),
                s.last_response_id.clone(),
                s.enabled_tools.clone(),
            )
        };

        let model = model.as_deref().unwrap_or(&self.default_model).to_string();
        let api_key = api_key
            .or_else(|| self.global_api_key.clone())
            .ok_or_else(|| {
                internal_error("no API key for session — set XAI_API_KEY or authenticate first")
            })?;

        // Build initial input and previous_response_id.
        //
        // When a `last_response_id` is stored, the xAI server already holds the
        // full prior context — send only the new user message and reference the
        // prior response. This avoids re-sending the entire history on every turn.
        //
        // When there is no `last_response_id` (new session, forked session, or
        // first turn), send the complete history converted to `InputItem`s.
        //
        // These are `mut` because the stale-ID retry and Incomplete continuation
        // may update them across outer loop iterations.
        let (mut current_input, mut current_prev_response_id) =
            if let Some(prev_id) = &last_response_id {
                (vec![InputItem::user(user_input.clone())], Some(prev_id.clone()))
            } else {
                (
                    build_full_history_input(
                        self.system_prompt.as_deref(),
                        &history,
                        &user_input,
                    ),
                    None,
                )
            };

        // Register a cancel channel so cancel() can abort this prompt.
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        self.cancel_senders
            .lock()
            .await
            .insert(session_id.clone(), cancel_tx);

        let client = Arc::clone(&self.client);
        let mut assistant_text = String::new();
        let mut canceled = false;
        let mut pending_tool_calls: Vec<(String, String)> = Vec::new();
        // ID returned by xAI for this response; saved as `last_response_id` so
        // subsequent turns can use stateful multi-turn via `previous_response_id`.
        let mut current_response_id: Option<String> = None;
        // Set after the first stale-ID retry to prevent infinite retries.
        let mut stale_retry_done = false;
        // Set when the outer loop is executing a continuation request (Incomplete
        // response). Disables the stale-ID retry for that iteration.
        let mut continuation_in_progress = false;
        // Counts client-driven continuation requests for Incomplete responses.
        let mut continuations: u32 = 0;
        const MAX_CONTINUATIONS: u32 = 5;

        // Outer loop — normally executes once. Re-runs when:
        //   Stale-ID retry: a stale `previous_response_id` causes an error →
        //          retry with full history and no ID (transparent recovery).
        //   Incomplete continuation: the model response is Incomplete → send a
        //          follow-up request referencing the partial response so the
        //          model continues.
        let stop_reason = 'outer: loop {
            let mut stream = client
                .chat_stream(
                    &model,
                    &current_input,
                    &api_key,
                    &enabled_tools,
                    current_prev_response_id.as_deref(),
                    self.max_turns,
                )
                .await;

            // Tracks whether the last Finished event signalled Incomplete.
            let mut needs_continuation = false;
            // The `incomplete_details.reason` from the last Finished(Incomplete) event.
            let mut last_incomplete_reason: Option<String> = None;

            let inner_stop = loop {
                let event = tokio::select! {
                    biased;
                    _ = &mut cancel_rx => {
                        info!(session_id, "xai: prompt canceled");
                        canceled = true;
                        break 'outer StopReason::Cancelled;
                    }
                    maybe = tokio::time::timeout(self.prompt_timeout, stream.next()) => {
                        match maybe {
                            Err(_elapsed) => {
                                warn!(session_id, "xai: prompt timed out");
                                // Do NOT set canceled=true. Unlike an explicit cancel,
                                // a timeout may have produced partial text that should be
                                // preserved.
                                break 'outer StopReason::Cancelled;
                            }
                            Ok(Some(e)) => e,
                            Ok(None) => break StopReason::EndTurn,
                        }
                    }
                };

                match event {
                    XaiEvent::TextDelta { text } => {
                        assistant_text.push_str(&text);
                        let notif = SessionNotification::new(
                            session_id.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::from(text),
                            )),
                        );
                        self.notifier.notify(notif).await;
                    }
                    XaiEvent::ResponseId { id } => {
                        current_response_id = Some(id);
                    }
                    XaiEvent::FunctionCall { call_id, name, arguments } => {
                        // NOTE: grok-4 (the default model) executes server-side tools
                        // transparently without emitting FunctionCall SSE events. This
                        // branch fires only with models that do surface tool calls in the
                        // stream (e.g. custom function calling). The ToolCall(Pending)
                        // notification will therefore not fire in the typical xAI deployment.
                        info!(session_id, call_id = %call_id, tool_name = %name, "xai: tool call");
                        let tool_call = ToolCall::new(call_id.clone(), name.clone())
                            .status(ToolCallStatus::Pending)
                            .kind(ToolKind::Other)
                            .raw_input(parse_tool_arguments(&arguments));
                        let notif = SessionNotification::new(
                            session_id.clone(),
                            SessionUpdate::ToolCall(tool_call),
                        );
                        self.notifier.notify(notif).await;
                        pending_tool_calls.push((call_id, name));
                    }
                    XaiEvent::ServerToolCompleted { name } => {
                        // NOTE: grok-4 does not emit this event — see FunctionCall note above.
                        if let Some(pos) = pending_tool_calls.iter().position(|(_, n)| *n == name) {
                            let (call_id, tc_name) = pending_tool_calls.remove(pos);
                            info!(session_id, call_id = %call_id, tool_name = %tc_name,
                                  "xai: server tool completed");
                            let notif = SessionNotification::new(
                                session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new(call_id, tc_name)
                                        .status(ToolCallStatus::Completed)
                                        .kind(ToolKind::Other),
                                ),
                            );
                            self.notifier.notify(notif).await;
                        }
                    }
                    XaiEvent::Finished { reason, incomplete_reason } => {
                        info!(session_id, reason = ?reason, incomplete_reason = ?incomplete_reason,
                              "xai: finish reason");
                        match reason {
                            FinishReason::Incomplete => {
                                needs_continuation = true;
                                last_incomplete_reason = incomplete_reason;
                            }
                            FinishReason::Failed => {
                                warn!(session_id, "xai: response failed");
                                break StopReason::EndTurn;
                            }
                            FinishReason::Cancelled => {
                                info!(session_id, "xai: response cancelled by server");
                                break StopReason::Cancelled;
                            }
                            FinishReason::Completed => {}
                            FinishReason::Other(ref s) => {
                                warn!(session_id, status = %s,
                                      "xai: unknown finish status — treating as end of turn");
                            }
                        }
                    }
                    XaiEvent::Usage { prompt_tokens, completion_tokens } => {
                        info!(session_id, prompt_tokens, completion_tokens, "xai: token usage");
                        let notif = SessionNotification::new(
                            session_id.clone(),
                            SessionUpdate::UsageUpdate(UsageUpdate::new(prompt_tokens, completion_tokens)),
                        );
                        self.notifier.notify(notif).await;
                    }
                    XaiEvent::Done => {
                        break StopReason::EndTurn;
                    }
                    XaiEvent::Error { message } => {
                        // If we used a session-carried previous_response_id and get an
                        // error on the first attempt, retry transparently with full history
                        // — the stored ID may have expired. Only fires once and never
                        // during continuations.
                        //
                        // 4xx errors (auth failure, bad request, quota) are NOT retried.
                        let is_client_error = message.contains("xAI API error 4");
                        if current_prev_response_id.is_some()
                            && !stale_retry_done
                            && !continuation_in_progress
                            && !is_client_error
                        {
                            warn!(session_id, error = %message,
                                  "xai: error with previous_response_id — retrying with full history");
                            stale_retry_done = true;
                            // Clear any text accumulated before the error.
                            assistant_text.clear();
                            // Clear any pending tool calls from the failed attempt.
                            pending_tool_calls.clear();
                            // Clear response ID from the failed request.
                            current_response_id = None;
                            current_prev_response_id = None;
                            current_input = build_full_history_input(
                                self.system_prompt.as_deref(),
                                &history,
                                &user_input,
                            );
                            continue 'outer;
                        }
                        tracing::error!(session_id, error = %message, "xai: stream error");
                        break StopReason::EndTurn;
                    }
                }
            };

            // If the model was cut short, send a follow-up request so it continues.
            if needs_continuation {
                if continuations >= MAX_CONTINUATIONS {
                    warn!(
                        session_id,
                        MAX_CONTINUATIONS,
                        "xai: max continuations reached — returning partial response as Cancelled"
                    );
                    break 'outer StopReason::Cancelled;
                }
                if let Some(resp_id) = current_response_id.clone() {
                    info!(
                        session_id,
                        continuations,
                        incomplete_reason = ?last_incomplete_reason,
                        "xai: response incomplete — sending continuation request"
                    );
                    current_prev_response_id = Some(resp_id);
                    current_input = match last_incomplete_reason.as_deref() {
                        Some("max_output_tokens") => vec![],
                        _ => vec![InputItem::user(user_input.clone())],
                    };
                    continuations += 1;
                    continuation_in_progress = true;
                    continue 'outer;
                } else {
                    warn!(
                        session_id,
                        incomplete_reason = ?last_incomplete_reason,
                        "xai: response incomplete but no response_id received — returning truncated response"
                    );
                }
            }

            break 'outer inner_stop;
        };

        // Remove cancel channel (sender may already be removed by cancel()).
        self.cancel_senders.lock().await.remove(&session_id);

        // Update session history.
        //
        // On explicit cancel: discard this turn entirely (no user message pushed).
        // On timeout, stream error, or success: push user + optional assistant, then trim.
        if !canceled {
            let mut sessions = self.sessions.lock().await;
            if let Some(s) = sessions.get_mut(&session_id) {
                s.history.push(Message::user(user_input));
                if !assistant_text.is_empty() {
                    s.history.push(Message::assistant_text(assistant_text));
                }
                trim_history(&mut s.history, self.max_history);
                // Only update the response ID if we received one; keep the old
                // ID when the stream ended without producing a new one.
                if let Some(resp_id) = current_response_id {
                    s.last_response_id = Some(resp_id);
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

/// Build the full `input` array for a Responses API request from session history.
///
/// Used when there is no `previous_response_id` (new session, forked session, or
/// fallback after a stale-ID error). Prepends the system prompt if present, then
/// converts each `Message` in history to the corresponding `InputItem` variant.
fn build_full_history_input(
    system_prompt: Option<&str>,
    history: &[Message],
    user_input: &str,
) -> Vec<InputItem> {
    let mut items: Vec<InputItem> = Vec::with_capacity(history.len() + 2);
    if let Some(sp) = system_prompt {
        items.push(InputItem::system(sp));
    }
    for msg in history {
        if msg.role == "assistant" {
            items.push(InputItem::assistant(msg.content_str()));
        } else {
            // Non-assistant roles (user, system injected via history, etc.) are
            // mapped to the user role — the Responses API only supports user/assistant.
            items.push(InputItem::user(msg.content_str()));
        }
    }
    items.push(InputItem::user(user_input.to_string()));
    items
}

/// Trim `history` in-place so it contains at most `max` messages.
///
/// Messages are removed from the front in structure-aware increments:
/// - A leading `user` + `assistant` pair is removed together (2 at a time).
/// - A leading orphaned `user` message (no corresponding assistant — left over
///   from a timed-out or cancelled turn) is removed individually (1 at a time).
/// - Any other leading role (malformed state) stops trimming immediately.
///
/// This avoids the blind "round up to even" approach, which could leave an
/// `assistant` message without a preceding `user` when orphaned user messages
/// accumulate from consecutive timed-out turns.
fn trim_history(history: &mut Vec<Message>, max: usize) {
    while history.len() > max {
        match (
            history.first().map(|m| m.role.as_str()),
            history.get(1).map(|m| m.role.as_str()),
        ) {
            (Some("user"), Some("assistant")) => {
                history.drain(..2);
            }
            (Some("user"), _) => {
                // Orphaned user message (followed by another user, or last in list).
                history.drain(..1);
            }
            _ => break, // Unexpected structure — stop to avoid corruption.
        }
    }
}

/// Parse the `arguments` string returned by xAI's API into a `serde_json::Value`.
///
/// xAI (like OpenAI) encodes tool call arguments as a JSON string
/// (e.g. `"{\"q\":\"test\"}"`) rather than an inline object. Parsing it here
/// means `ToolCall::raw_input` carries a proper JSON object instead of a
/// double-encoded string. Falls back to `Value::String` when the input is not
/// valid JSON.
fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    serde_json::from_str(arguments)
        .unwrap_or_else(|_| serde_json::Value::String(arguments.to_string()))
}

// ── Test helpers ──────────────────────────────────────────────────────────────

#[cfg(test)]
impl<H: XaiHttpClient, N: SessionNotifier> XaiAgent<H, N> {
    async fn test_insert_session(&self, id: &str, cwd: &str, model: Option<String>) {
        self.sessions.lock().await.insert(
            id.to_string(),
            XaiSession {
                cwd: cwd.to_string(),
                model,
                api_key: Some("test-key".to_string()),
                history: Vec::new(),
                last_response_id: None,
                enabled_tools: Vec::new(),
                system_prompt: None,
                created_at: Instant::now(),
            },
        );
    }

    async fn test_session_model(&self, id: &str) -> Option<String> {
        self.sessions.lock().await.get(id).and_then(|s| s.model.clone())
    }

    async fn test_session_cwd(&self, id: &str) -> Option<String> {
        self.sessions.lock().await.get(id).map(|s| s.cwd.clone())
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

    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.prompt_timeout = timeout;
        self
    }

    fn with_max_history(mut self, max_history: usize) -> Self {
        self.max_history = max_history;
        self
    }

    async fn test_insert_session_with_response_id(
        &self,
        id: &str,
        cwd: &str,
        model: Option<String>,
        response_id: Option<String>,
    ) {
        self.sessions.lock().await.insert(
            id.to_string(),
            XaiSession {
                cwd: cwd.to_string(),
                model,
                api_key: Some("test-key".to_string()),
                history: Vec::new(),
                last_response_id: response_id,
                enabled_tools: Vec::new(),
                system_prompt: None,
                created_at: Instant::now(),
            },
        );
    }

    async fn test_insert_session_no_key(&self, id: &str, cwd: &str) {
        self.sessions.lock().await.insert(
            id.to_string(),
            XaiSession {
                cwd: cwd.to_string(),
                model: None,
                api_key: None,
                history: Vec::new(),
                last_response_id: None,
                enabled_tools: Vec::new(),
                system_prompt: None,
                created_at: Instant::now(),
            },
        );
    }

    async fn test_insert_session_with_history(&self, id: &str, cwd: &str, history: Vec<Message>) {
        self.sessions.lock().await.insert(
            id.to_string(),
            XaiSession {
                cwd: cwd.to_string(),
                model: None,
                api_key: Some("test-key".to_string()),
                history,
                last_response_id: None,
                enabled_tools: Vec::new(),
                system_prompt: None,
                created_at: Instant::now(),
            },
        );
    }

    async fn test_last_response_id(&self, id: &str) -> Option<String> {
        self.sessions.lock().await.get(id).and_then(|s| s.last_response_id.clone())
    }

    async fn test_pending_api_key(&self) -> Option<String> {
        self.pending_api_key.lock().await.clone()
    }

    async fn test_session_api_key(&self, id: &str) -> Option<String> {
        self.sessions.lock().await.get(id).and_then(|s| s.api_key.clone())
    }

    async fn test_session_history(&self, id: &str) -> Vec<Message> {
        self.sessions.lock().await.get(id).map(|s| s.history.clone()).unwrap_or_default()
    }

    fn test_max_history(&self) -> usize {
        self.max_history
    }

    async fn test_session_enabled_tools(&self, id: &str) -> Vec<String> {
        self.sessions.lock().await.get(id).map(|s| s.enabled_tools.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_client_protocol::{
        Agent, AuthMethodId, AuthenticateRequest, CancelNotification, CloseSessionRequest,
        ContentBlock, ForkSessionRequest, InitializeRequest, ListSessionsRequest,
        LoadSessionRequest, NewSessionRequest, PromptRequest, ProtocolVersion, ResumeSessionRequest,
        SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
        SetSessionModelRequest,
    };

    use agent_client_protocol::StopReason;

    use super::*;
    use crate::client::{FinishReason, XaiEvent};
    use crate::http_client::mock::MockXaiHttpClient;
    use crate::session_notifier::MockSessionNotifier;

    type TestAgent = XaiAgent<Arc<MockXaiHttpClient>, Arc<MockSessionNotifier>>;

    fn make_agent() -> TestAgent {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        XaiAgent::with_deps(mock_notifier, "grok-3", "test-key", mock_http)
    }

    // ── close_session ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn close_session_removes_session() {
        let agent = make_agent();
        agent.test_insert_session("s1", "/tmp", None).await;
        assert_eq!(agent.test_session_count().await, 1);
        agent.close_session(CloseSessionRequest::new("s1")).await.unwrap();
        assert_eq!(agent.test_session_count().await, 0);
    }

    #[tokio::test]
    async fn close_session_unknown_id_is_noop() {
        let agent = make_agent();
        agent.close_session(CloseSessionRequest::new("nonexistent")).await.unwrap();
    }

    // ── load_session ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn load_session_returns_state() {
        let agent = make_agent();
        agent
            .test_insert_session("s2", "/home/user", Some("grok-3-mini".to_string()))
            .await;
        let resp = agent
            .load_session(LoadSessionRequest::new("s2", "/home/user"))
            .await
            .unwrap();
        assert_eq!(resp.models.unwrap().current_model_id.to_string(), "grok-3-mini");
    }

    #[tokio::test]
    async fn load_session_not_found_returns_error() {
        let agent = make_agent();
        assert!(agent.load_session(LoadSessionRequest::new("missing", "/")).await.is_err());
    }

    // ── resume_session ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resume_session_returns_error_for_unknown_session() {
        let agent = make_agent();
        let err = agent
            .resume_session(ResumeSessionRequest::new("nonexistent", "/"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    // ── fork_session ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fork_session_inherits_model() {
        let agent = make_agent();
        agent
            .test_insert_session("src", "/tmp", Some("grok-3-mini".to_string()))
            .await;
        let resp = agent
            .fork_session(ForkSessionRequest::new("src", "/fork"))
            .await
            .unwrap();
        let new_id = resp.session_id.to_string();
        assert_eq!(
            agent.test_session_model(&new_id).await.as_deref(),
            Some("grok-3-mini")
        );
    }

    #[tokio::test]
    async fn fork_session_returns_error_for_unknown_source() {
        let agent = make_agent();
        let err = agent
            .fork_session(ForkSessionRequest::new("nonexistent", "/fork"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    // ── set_session_model ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_model_updates_model() {
        // Hold env_lock so XAI_MODELS changes in other tests don't remove grok-3-mini.
        let _guard = env_lock().lock().unwrap();
        let agent = make_agent();
        agent.test_insert_session("s3", "/tmp", None).await;
        agent
            .set_session_model(SetSessionModelRequest::new("s3", "grok-3-mini"))
            .await
            .unwrap();
        assert_eq!(
            agent.test_session_model("s3").await.as_deref(),
            Some("grok-3-mini")
        );
    }

    #[tokio::test]
    async fn set_session_model_rejects_unknown_model() {
        let agent = make_agent();
        agent.test_insert_session("s4", "/tmp", None).await;
        let err = agent
            .set_session_model(SetSessionModelRequest::new("s4", "gpt-99"))
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown model"));
    }

    // ── set_session_model clears last_response_id ─────────────────────────────

    #[tokio::test]
    async fn set_session_model_clears_last_response_id() {
        let _guard = env_lock().lock().unwrap();
        let agent = make_agent();
        agent
            .test_insert_session_with_response_id("s_mid", "/tmp", None, Some("old-id".to_string()))
            .await;
        agent
            .set_session_model(SetSessionModelRequest::new("s_mid", "grok-3-mini"))
            .await
            .unwrap();
        assert_eq!(
            agent.test_last_response_id("s_mid").await,
            None,
            "set_session_model must clear last_response_id to prevent stale-ID errors"
        );
    }

    // ── list_sessions ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_returns_sorted() {
        let agent = make_agent();
        agent.test_insert_session("zzz", "/c", None).await;
        agent.test_insert_session("aaa", "/a", None).await;
        agent.test_insert_session("mmm", "/b", None).await;
        let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
        let ids: Vec<_> = resp
            .sessions
            .iter()
            .map(|s| s.session_id.to_string())
            .collect();
        assert_eq!(ids, vec!["aaa", "mmm", "zzz"]);
    }

    #[tokio::test]
    async fn list_sessions_empty() {
        let agent = make_agent();
        let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
        assert!(resp.sessions.is_empty());
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn initialize_returns_latest_protocol_version() {
        let agent = make_agent();
        let resp = agent
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await
            .unwrap();
        assert_eq!(resp.protocol_version, ProtocolVersion::LATEST);
    }

    #[tokio::test]
    async fn initialize_advertises_session_capabilities() {
        let agent = make_agent();
        let resp = agent
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await
            .unwrap();
        let sc = resp.agent_capabilities.session_capabilities;
        assert!(sc.fork.is_some());
        assert!(sc.list.is_some());
        assert!(sc.resume.is_some());
        assert!(sc.close.is_some());
    }

    #[tokio::test]
    async fn initialize_includes_agent_info() {
        let agent = make_agent();
        let resp = agent
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await
            .unwrap();
        let info = resp.agent_info.expect("agent_info should be present");
        assert_eq!(info.name, "trogon-xai-runner");
    }

    // ── authenticate ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn authenticate_always_succeeds() {
        let agent = make_agent();
        agent
            .authenticate(AuthenticateRequest::new(AuthMethodId::from("any-method")))
            .await
            .unwrap();
    }

    // ── set_session_mode ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_mode_succeeds_for_default_mode() {
        let agent = make_agent();
        agent.test_insert_session("sm1", "/tmp", None).await;
        agent
            .set_session_mode(SetSessionModeRequest::new("sm1", "default"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn set_session_mode_rejects_unknown_mode() {
        let agent = make_agent();
        agent.test_insert_session("sm2", "/tmp", None).await;
        let err = agent
            .set_session_mode(SetSessionModeRequest::new("sm2", "whatever-mode"))
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown mode"), "error: {}", err.message);
    }

    // ── set_session_config_option ─────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_config_option_unknown_option_returns_empty() {
        // An unknown config option is ignored and returns empty config_options.
        let agent = make_agent();
        agent.test_insert_session("cfg1", "/tmp", None).await;
        let resp: SetSessionConfigOptionResponse = agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "cfg1",
                "some-unknown-option",
                "some-value",
            ))
            .await
            .unwrap();
        assert!(resp.config_options.is_empty(), "unknown option must return empty config_options");
    }

    #[tokio::test]
    async fn set_session_config_option_unknown_session_returns_ok() {
        // Unknown session is tolerated — returns empty config_options without error.
        let agent = make_agent();
        let resp = agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "no-such-session",
                "some-option",
                "some-value",
            ))
            .await
            .unwrap();
        assert!(resp.config_options.is_empty());
    }

    #[tokio::test]
    async fn set_session_config_option_toggles_web_search_on() {
        let agent = make_agent();
        agent.test_insert_session("cfg2", "/tmp", None).await;

        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "cfg2",
                "web_search",
                "on",
            ))
            .await
            .unwrap();

        // enabled_tools should now contain "web_search".
        let enabled = agent.test_session_enabled_tools("cfg2").await;
        assert!(enabled.contains(&"web_search".to_string()), "web_search must be enabled");
    }

    #[tokio::test]
    async fn set_session_config_option_toggles_web_search_off() {
        let agent = make_agent();
        agent.test_insert_session("cfg3", "/tmp", None).await;

        // Turn on first.
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("cfg3", "web_search", "on"))
            .await
            .unwrap();

        // Turn off.
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "cfg3",
                "web_search",
                "off",
            ))
            .await
            .unwrap();

        let enabled = agent.test_session_enabled_tools("cfg3").await;
        assert!(!enabled.contains(&"web_search".to_string()), "web_search must be disabled");
    }

    // ── prompt ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_returns_error_for_unknown_session() {
        let agent = make_agent();
        let err = agent
            .prompt(PromptRequest::new("unknown-session", vec![]))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    #[tokio::test]
    async fn prompt_appends_history_after_turn() {
        let agent = make_agent();
        agent.test_insert_session("h1", "/tmp", None).await;

        agent.client.push_response(vec![
            XaiEvent::TextDelta { text: "hello".to_string() },
            XaiEvent::Done,
        ]);

        agent
            .prompt(PromptRequest::new(
                "h1",
                vec![ContentBlock::from("hi".to_string())],
            ))
            .await
            .unwrap();

        // user + assistant = 2 history entries
        assert_eq!(agent.test_history_len("h1").await, 2);
    }

    #[tokio::test]
    async fn prompt_sends_notification_for_text_delta() {
        let agent = make_agent();
        agent.test_insert_session("n1", "/tmp", None).await;

        agent.client.push_response(vec![
            XaiEvent::TextDelta { text: "world".to_string() },
            XaiEvent::Done,
        ]);

        agent
            .prompt(PromptRequest::new(
                "n1",
                vec![ContentBlock::from("hi".to_string())],
            ))
            .await
            .unwrap();

        let notifs = agent.notifier.notifications.lock().unwrap();
        assert_eq!(notifs.len(), 1, "expected one notification");
    }

    // ── cancel ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_noop_for_unknown_session() {
        let agent = make_agent();
        agent.cancel(CancelNotification::new("no-such-session")).await.unwrap();
    }

    // ── model list ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn default_model_added_when_not_in_list() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent = XaiAgent::with_deps(mock_notifier, "custom-model", "key", mock_http);
        let state = agent.session_model_state(None);
        let ids: Vec<_> =
            state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        assert!(ids.contains(&"custom-model".to_string()), "available: {ids:?}");
        assert_eq!(state.current_model_id.to_string(), "custom-model");
    }

    // ── session_mode_state ────────────────────────────────────────────────────

    #[tokio::test]
    async fn session_mode_state_current_is_default() {
        let agent = make_agent();
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
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent = XaiAgent::with_deps(mock_notifier, "grok-3", "key", mock_http);
        unsafe { std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS") };
        assert_eq!(agent.test_prompt_timeout(), Duration::from_secs(300));
    }

    // ── new_session ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn new_session_creates_session() {
        let agent = make_agent();
        agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
        assert_eq!(agent.test_session_count().await, 1);
    }

    #[tokio::test]
    async fn new_session_falls_back_to_global_api_key() {
        // make_agent passes "test-key" as the global API key.
        let agent = make_agent();
        let resp = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
        let session_id = resp.session_id.to_string();
        assert_eq!(agent.test_session_api_key(&session_id).await.as_deref(), Some("test-key"));
    }

    #[tokio::test]
    async fn new_session_uses_pending_api_key() {
        // Agent with no global key; key must come from authenticate().
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "", Arc::clone(&mock_http));

        let mut meta = serde_json::Map::new();
        meta.insert("XAI_API_KEY".to_string(), serde_json::json!("user-key"));
        agent.authenticate(AuthenticateRequest::new("api-key").meta(meta)).await.unwrap();

        let resp = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
        let session_id = resp.session_id.to_string();

        assert_eq!(agent.test_session_api_key(&session_id).await.as_deref(), Some("user-key"));
        assert_eq!(
            agent.test_pending_api_key().await,
            None,
            "pending key must be consumed by new_session"
        );
    }

    // ── fork: api_key inherited from source ──────────────────────────────────

    #[tokio::test]
    async fn fork_inherits_session_api_key_from_source() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        // Agent with no global key — every session must carry its own key.
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "", Arc::clone(&mock_http));

        // Authenticate to set a pending key, then create the source session.
        let mut meta = serde_json::Map::new();
        meta.insert("XAI_API_KEY".to_string(), serde_json::json!("session-key"));
        agent.authenticate(AuthenticateRequest::new("api-key").meta(meta)).await.unwrap();
        let resp = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
        let src_id = resp.session_id.to_string();
        assert_eq!(agent.test_session_api_key(&src_id).await.as_deref(), Some("session-key"));

        // Fork the source session.
        let fork_resp =
            agent.fork_session(ForkSessionRequest::new(src_id.clone(), "/fork")).await.unwrap();
        let fork_id = fork_resp.session_id.to_string();

        assert_eq!(
            agent.test_session_api_key(&fork_id).await.as_deref(),
            Some("session-key"),
            "fork must inherit the source session's per-session api_key"
        );
    }

    // ── fork: last_response_id is NOT inherited ───────────────────────────────

    #[tokio::test]
    async fn fork_does_not_inherit_last_response_id() {
        let agent = make_agent();
        agent
            .test_insert_session_with_response_id(
                "src",
                "/tmp",
                None,
                Some("src-resp-id".to_string()),
            )
            .await;

        let resp = agent.fork_session(ForkSessionRequest::new("src", "/fork")).await.unwrap();
        let fork_id = resp.session_id.to_string();

        assert_eq!(
            agent.test_last_response_id(&fork_id).await,
            None,
            "fork must reset last_response_id to None even when source had one"
        );
    }

    // ── authenticate: no meta does not set pending key ────────────────────────

    #[tokio::test]
    async fn authenticate_with_no_meta_does_not_set_pending_key() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "", Arc::clone(&mock_http));

        agent.authenticate(AuthenticateRequest::new("api-key")).await.unwrap();

        assert_eq!(
            agent.test_pending_api_key().await,
            None,
            "authenticate without meta must not set pending_api_key"
        );
    }

    // ── authenticate: non-string XAI_API_KEY is ignored ─────────────────────

    #[tokio::test]
    async fn authenticate_with_non_string_xai_api_key_does_not_set_pending_key() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "", Arc::clone(&mock_http));

        let mut meta = serde_json::Map::new();
        meta.insert("XAI_API_KEY".to_string(), serde_json::json!(42));
        agent.authenticate(AuthenticateRequest::new("api-key").meta(meta)).await.unwrap();

        assert_eq!(
            agent.test_pending_api_key().await,
            None,
            "non-string XAI_API_KEY must be rejected by .as_str() filter"
        );
    }

    #[tokio::test]
    async fn new_session_second_call_falls_back_to_global_key_after_pending_consumed() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent = XaiAgent::with_deps(
            Arc::clone(&mock_notifier),
            "grok-3",
            "global-key",
            Arc::clone(&mock_http),
        );

        // Authenticate to set a pending key.
        let mut meta = serde_json::Map::new();
        meta.insert("XAI_API_KEY".to_string(), serde_json::json!("pending-key"));
        agent.authenticate(AuthenticateRequest::new("api-key").meta(meta)).await.unwrap();

        // First new_session — consumes the pending key.
        let resp1 = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
        let sid1 = resp1.session_id.to_string();
        assert_eq!(agent.test_session_api_key(&sid1).await.as_deref(), Some("pending-key"));
        assert_eq!(agent.test_pending_api_key().await, None, "pending key must be consumed");

        // Second new_session — no pending key; must fall back to global_api_key.
        let resp2 = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
        let sid2 = resp2.session_id.to_string();
        assert_eq!(
            agent.test_session_api_key(&sid2).await.as_deref(),
            Some("global-key"),
            "second new_session must use global_api_key after pending key is consumed"
        );
    }

    // ── authenticate stores key ───────────────────────────────────────────────

    #[tokio::test]
    async fn authenticate_stores_pending_api_key() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "", Arc::clone(&mock_http));

        let mut meta = serde_json::Map::new();
        meta.insert("XAI_API_KEY".to_string(), serde_json::json!("my-key"));
        agent.authenticate(AuthenticateRequest::new("api-key").meta(meta)).await.unwrap();

        assert_eq!(agent.test_pending_api_key().await.as_deref(), Some("my-key"));
    }

    // ── prompt: missing API key ───────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_fails_when_no_api_key() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "", Arc::clone(&mock_http));
        agent.test_insert_session_no_key("nokey", "/tmp").await;

        let err = agent
            .prompt(PromptRequest::new("nokey", vec![ContentBlock::from("hi")]))
            .await
            .unwrap_err();
        assert!(err.message.contains("no API key"), "error: {}", err.message);
    }

    // ── prompt: previous_response_id shortcut ─────────────────────────────────

    #[tokio::test]
    async fn prompt_uses_previous_response_id() {
        let agent = make_agent();
        agent
            .test_insert_session_with_response_id("p1", "/tmp", None, Some("prev-id".to_string()))
            .await;
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("p1", vec![ContentBlock::from("follow-up")]))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        let call = calls.last().unwrap();
        assert_eq!(call.previous_response_id.as_deref(), Some("prev-id"));
        // Only the new user message is sent — no history replay.
        assert_eq!(
            call.input.len(),
            1,
            "only new user message when previous_response_id is set"
        );
    }

    // ── prompt: response ID stored after turn ─────────────────────────────────

    #[tokio::test]
    async fn prompt_stores_response_id_after_turn() {
        let agent = make_agent();
        agent.test_insert_session("r1", "/tmp", None).await;
        agent.client.push_response(vec![
            XaiEvent::ResponseId { id: "resp-abc".to_string() },
            XaiEvent::Done,
        ]);

        agent
            .prompt(PromptRequest::new("r1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        assert_eq!(
            agent.test_last_response_id("r1").await.as_deref(),
            Some("resp-abc"),
            "last_response_id must be updated after a turn"
        );
    }

    // ── prompt: history trimming ──────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_trims_history_when_max_exceeded() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent = XaiAgent::with_deps(
            Arc::clone(&mock_notifier),
            "grok-3",
            "test-key",
            Arc::clone(&mock_http),
        )
        .with_max_history(4);
        agent.test_insert_session("trim", "/tmp", None).await;

        // 3 turns produce 6 history entries (user + assistant each); must be trimmed to 4.
        for _ in 0..3 {
            mock_http.push_response(vec![
                XaiEvent::TextDelta { text: "reply".to_string() },
                XaiEvent::Done,
            ]);
            agent
                .prompt(PromptRequest::new("trim", vec![ContentBlock::from("hi")]))
                .await
                .unwrap();
        }

        let len = agent.test_history_len("trim").await;
        assert!(len <= 4, "history ({len}) should be trimmed to max_history=4");
    }

    // ── prompt: timeout ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_timeout_fires() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent = XaiAgent::with_deps(
            Arc::clone(&mock_notifier),
            "grok-3",
            "test-key",
            Arc::clone(&mock_http),
        )
        .with_timeout(Duration::from_millis(50));
        agent.test_insert_session("slow", "/tmp", None).await;

        // Yields one event then blocks forever — agent timeout must fire and return.
        mock_http.push_slow_response(XaiEvent::TextDelta { text: "partial".to_string() });

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            agent.prompt(PromptRequest::new("slow", vec![ContentBlock::from("hi")])),
        )
        .await;

        assert!(result.is_ok(), "prompt must return after timeout, not hang indefinitely");
    }

    // ── fork_session: copies history and clears response id ──────────────────

    #[tokio::test]
    async fn fork_session_copies_history() {
        let agent = make_agent();
        let history = vec![Message::user("hello"), Message::assistant_text("hi there")];
        agent.test_insert_session_with_history("src3", "/a", history).await;

        let resp = agent.fork_session(ForkSessionRequest::new("src3", "/b")).await.unwrap();
        let fork_id = resp.session_id.to_string();

        let fork_history = agent.test_session_history(&fork_id).await;
        assert_eq!(fork_history.len(), 2);
        assert_eq!(fork_history[0].role, "user");
        assert_eq!(fork_history[1].role, "assistant");
    }

    #[tokio::test]
    async fn fork_session_clears_response_id() {
        let agent = make_agent();
        agent
            .test_insert_session_with_response_id("src4", "/c", None, Some("old-id".to_string()))
            .await;

        let resp = agent.fork_session(ForkSessionRequest::new("src4", "/d")).await.unwrap();
        let fork_id = resp.session_id.to_string();

        assert_eq!(
            agent.test_last_response_id(&fork_id).await,
            None,
            "fork must start without previous_response_id"
        );
    }

    // ── set_session_model: unknown session ────────────────────────────────────

    #[tokio::test]
    async fn set_session_model_unknown_session_returns_error() {
        let agent = make_agent();
        let err = agent
            .set_session_model(SetSessionModelRequest::new("nonexistent", "grok-3"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"), "error: {}", err.message);
    }

    // ── resume_session: success path ──────────────────────────────────────────

    #[tokio::test]
    async fn resume_session_returns_ok_for_known_session() {
        let agent = make_agent();
        agent.test_insert_session("rs1", "/tmp", None).await;
        agent.resume_session(ResumeSessionRequest::new("rs1", "/tmp")).await.unwrap();
    }

    // ── fork_session: inherits API key ────────────────────────────────────────

    #[tokio::test]
    async fn fork_session_inherits_api_key() {
        let agent = make_agent();
        // test_insert_session sets api_key = "test-key".
        agent.test_insert_session("src5", "/tmp", None).await;

        let resp =
            agent.fork_session(ForkSessionRequest::new("src5", "/fork5")).await.unwrap();
        let fork_id = resp.session_id.to_string();

        assert_eq!(
            agent.test_session_api_key(&fork_id).await.as_deref(),
            Some("test-key"),
            "fork must inherit the source session's API key"
        );
    }

    // ── authenticate: ignores empty key ──────────────────────────────────────

    #[tokio::test]
    async fn authenticate_ignores_empty_api_key() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "", Arc::clone(&mock_http));

        let mut meta = serde_json::Map::new();
        meta.insert("XAI_API_KEY".to_string(), serde_json::json!(""));
        agent.authenticate(AuthenticateRequest::new("api-key").meta(meta)).await.unwrap();

        assert_eq!(
            agent.test_pending_api_key().await,
            None,
            "empty API key in meta must not be stored"
        );
    }

    // ── prompt: session model override ────────────────────────────────────────

    #[tokio::test]
    async fn prompt_uses_session_model_override() {
        let agent = make_agent(); // default_model = "grok-3"
        agent.test_insert_session("mo1", "/tmp", Some("grok-3-mini".to_string())).await;
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("mo1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        assert_eq!(calls.last().unwrap().model, "grok-3-mini");
    }

    // ── prompt: system prompt prepended ──────────────────────────────────────

    #[tokio::test]
    async fn prompt_prepends_system_prompt_when_set() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_SYSTEM_PROMPT", "You are a helpful assistant.") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent = XaiAgent::with_deps(
            Arc::clone(&mock_notifier),
            "grok-3",
            "test-key",
            Arc::clone(&mock_http),
        );
        unsafe { std::env::remove_var("XAI_SYSTEM_PROMPT") };

        agent.test_insert_session("sp1", "/tmp", None).await;
        mock_http.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("sp1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        let calls = mock_http.calls.lock().unwrap();
        let input = &calls.last().unwrap().input;
        assert_eq!(input[0].role, "system", "first input item must be the system prompt");
        assert_eq!(input[0].content, "You are a helpful assistant.");
    }

    // ── prompt: stream error returns Ok ──────────────────────────────────────────

    #[tokio::test]
    async fn prompt_stream_error_returns_ok() {
        let agent = make_agent();
        agent.test_insert_session("err1", "/tmp", None).await;
        agent.client.push_response(vec![
            XaiEvent::Error { message: "server exploded".to_string() },
        ]);

        // Stream errors return Ok(PromptResponse) — not Err — to keep the session alive.
        agent
            .prompt(PromptRequest::new("err1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn prompt_stream_error_preserves_last_response_id() {
        // When a stream error occurs, the session's `last_response_id` must remain
        // unchanged because the turn ended without producing a new response ID.
        let agent = make_agent();
        agent
            .test_insert_session_with_response_id(
                "err2",
                "/tmp",
                None,
                Some("old-resp-id".to_string()),
            )
            .await;
        // Use a 4xx error message to prevent stale-ID retry.
        agent.client.push_response(vec![
            XaiEvent::Error {
                message: "xAI API error 401: Unauthorized".to_string(),
            },
        ]);

        agent
            .prompt(PromptRequest::new("err2", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        assert_eq!(
            agent.test_last_response_id("err2").await.as_deref(),
            Some("old-resp-id"),
            "last_response_id must not be cleared when stream error produced no ResponseId"
        );
    }

    // ── prompt: no text response → only user in history ──────────────────────

    #[tokio::test]
    async fn prompt_without_text_response_only_records_user_message() {
        let agent = make_agent();
        agent.test_insert_session("notxt", "/tmp", None).await;
        // Only Done — no TextDelta — so assistant_text stays empty.
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("notxt", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        // Only the user message is recorded; no empty assistant entry created.
        assert_eq!(agent.test_history_len("notxt").await, 1);
    }

    // ── prompt: full history sent when no previous_response_id ───────────────

    #[tokio::test]
    async fn prompt_sends_full_history_when_no_previous_response_id() {
        let agent = make_agent();
        let history = vec![
            Message::user("first question"),
            Message::assistant_text("first answer"),
        ];
        agent.test_insert_session_with_history("hist1", "/tmp", history).await;
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new(
                "hist1",
                vec![ContentBlock::from("second question")],
            ))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        let call = calls.last().unwrap();
        // 2 history items + 1 new user message = 3 input items; no shortcut.
        assert_eq!(call.previous_response_id, None);
        assert_eq!(
            call.input.len(),
            3,
            "full history + new message sent when no previous_response_id"
        );
    }

    // ── prompt: max_turns passed to chat_stream ───────────────────────────────

    #[tokio::test]
    async fn prompt_passes_max_turns_to_chat_stream() {
        let agent = make_agent(); // max_turns defaults to Some(10)
        agent.test_insert_session("mt1", "/tmp", None).await;
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("mt1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        assert_eq!(calls.last().unwrap().max_turns, Some(10));
    }

    // ── prompt: multiple text blocks joined ───────────────────────────────────

    #[tokio::test]
    async fn prompt_joins_multiple_text_blocks_with_newline() {
        let agent = make_agent();
        agent.test_insert_session("mb1", "/tmp", None).await;
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new(
                "mb1",
                vec![ContentBlock::from("block one"), ContentBlock::from("block two")],
            ))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        let user_item = calls.last().unwrap().input.last().unwrap();
        assert_eq!(user_item.content, "block one\nblock two");
    }

    // ── XAI_MAX_HISTORY_MESSAGES env var ──────────────────────────────────────

    #[tokio::test]
    async fn max_history_env_var_parsed_correctly() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_MAX_HISTORY_MESSAGES", "5") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MAX_HISTORY_MESSAGES") };
        assert_eq!(agent.test_max_history(), 5);
    }

    #[tokio::test]
    async fn max_history_invalid_env_var_falls_back_to_default() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_MAX_HISTORY_MESSAGES", "not_a_number") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MAX_HISTORY_MESSAGES") };
        assert_eq!(agent.test_max_history(), 20);
    }

    #[tokio::test]
    async fn max_history_zero_falls_back_to_default() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_MAX_HISTORY_MESSAGES", "0") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MAX_HISTORY_MESSAGES") };
        assert_eq!(
            agent.test_max_history(),
            20,
            "XAI_MAX_HISTORY_MESSAGES=0 must fall back to the 20-entry default"
        );
    }

    // ── XAI_MODELS env var parsing ────────────────────────────────────────────

    #[tokio::test]
    async fn xai_models_env_var_parsed_correctly() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var(
                "XAI_MODELS",
                "grok-3:Grok 3,grok-3-mini:Grok 3 Mini,custom:Custom Model",
            );
        }
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MODELS") };

        let state = agent.session_model_state(None);
        let ids: Vec<_> =
            state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        assert!(ids.contains(&"grok-3".to_string()));
        assert!(ids.contains(&"grok-3-mini".to_string()));
        assert!(ids.contains(&"custom".to_string()));
        assert_eq!(ids.len(), 3);
    }

    #[tokio::test]
    async fn xai_models_env_var_skips_malformed_entries() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var(
                "XAI_MODELS",
                "grok-3:Grok 3,malformed-no-colon,another:Another",
            );
        }
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MODELS") };

        let state = agent.session_model_state(None);
        let ids: Vec<_> =
            state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        assert!(ids.contains(&"grok-3".to_string()), "valid entry must be present");
        assert!(ids.contains(&"another".to_string()), "valid entry must be present");
        assert!(
            !ids.iter().any(|id| id == "malformed-no-colon"),
            "malformed entry must be skipped"
        );
    }

    #[tokio::test]
    async fn xai_models_env_var_all_malformed_falls_back_to_defaults() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var("XAI_MODELS", "no-colon-here,also-no-colon");
        }
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MODELS") };

        let state = agent.session_model_state(None);
        let ids: Vec<_> =
            state.available_models.iter().map(|m| m.model_id.to_string()).collect();
        assert!(ids.contains(&"grok-3".to_string()), "default grok-3 must be present");
        assert!(ids.contains(&"grok-3-mini".to_string()), "default grok-3-mini must be present");
    }

    #[tokio::test]
    async fn xai_system_prompt_empty_string_treated_as_none() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_SYSTEM_PROMPT", "") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_SYSTEM_PROMPT") };

        agent.test_insert_session("spe1", "/tmp", None).await;
        mock_http.push_response(vec![XaiEvent::Done]);
        agent
            .prompt(PromptRequest::new("spe1", vec![ContentBlock::from("hello")]))
            .await
            .unwrap();

        let calls = mock_http.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let input = &calls[0].input;
        assert!(
            input[0].role != "system",
            "empty XAI_SYSTEM_PROMPT must not prepend a system input item"
        );
    }

    #[tokio::test]
    async fn xai_max_turns_zero_falls_back_to_default() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_MAX_TURNS", "0") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MAX_TURNS") };

        agent.test_insert_session("mtz1", "/tmp", None).await;
        mock_http.push_response(vec![XaiEvent::Done]);
        agent
            .prompt(PromptRequest::new("mtz1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        let calls = mock_http.calls.lock().unwrap();
        assert_eq!(
            calls.last().unwrap().max_turns,
            Some(10),
            "XAI_MAX_TURNS=0 must fall back to the default of 10"
        );
    }

    // ── prompt: usage events emit notifications; tool events do not ──────────────

    #[tokio::test]
    async fn prompt_emits_tool_call_usage_and_text_notifications() {
        // FunctionCall → ToolCall(Pending), ServerToolCompleted → ToolCall(Completed),
        // Usage → UsageUpdate, TextDelta → AgentMessageChunk = 4 notifications total.
        let agent = make_agent();
        agent.test_insert_session("ign1", "/tmp", None).await;
        agent.client.push_response(vec![
            XaiEvent::FunctionCall {
                call_id: "c1".to_string(),
                name: "web_search".to_string(),
                arguments: "{}".to_string(),
            },
            XaiEvent::ServerToolCompleted { name: "web_search".to_string() },
            XaiEvent::Usage { prompt_tokens: 10, completion_tokens: 5 },
            XaiEvent::TextDelta { text: "answer".to_string() },
            XaiEvent::Done,
        ]);

        agent
            .prompt(PromptRequest::new("ign1", vec![ContentBlock::from("search for rust")]))
            .await
            .unwrap();

        // user + assistant = 2 history entries
        assert_eq!(agent.test_history_len("ign1").await, 2);
        // ToolCall(Pending) + ToolCall(Completed) + UsageUpdate + AgentMessageChunk = 4
        let notifs = agent.notifier.notifications.lock().unwrap();
        assert_eq!(notifs.len(), 4, "expected 4 notifications: Pending, Completed, UsageUpdate, AgentMessageChunk");
    }

    // ── build_input: system prompt precedes history and new user message ──────

    #[tokio::test]
    async fn system_prompt_placed_before_history_and_new_user_message() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_SYSTEM_PROMPT", "You are concise.") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent = XaiAgent::with_deps(
            Arc::clone(&mock_notifier),
            "grok-3",
            "test-key",
            Arc::clone(&mock_http),
        );
        unsafe { std::env::remove_var("XAI_SYSTEM_PROMPT") };

        let history = vec![
            Message::user("previous question"),
            Message::assistant_text("previous answer"),
        ];
        agent.test_insert_session_with_history("sp2", "/tmp", history).await;
        mock_http.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("sp2", vec![ContentBlock::from("follow-up")]))
            .await
            .unwrap();

        let calls = mock_http.calls.lock().unwrap();
        let input = &calls.last().unwrap().input;
        assert_eq!(input.len(), 4, "system + user + assistant + new_user = 4 items");
        assert_eq!(input[0].role, "system", "item[0] must be the system prompt");
        assert_eq!(input[0].content, "You are concise.");
        assert_eq!(input[1].role, "user", "item[1] must be history user message");
        assert_eq!(input[2].role, "assistant", "item[2] must be history assistant message");
        assert_eq!(input[3].role, "user", "item[3] must be the new user message");
        assert_eq!(input[3].content, "follow-up");
    }

    // ── prompt: ResourceLink content block ────────────────────────────────────

    #[tokio::test]
    async fn prompt_with_resource_link_includes_reference_in_input() {
        use agent_client_protocol::ResourceLink;

        let agent = make_agent();
        agent.test_insert_session("ntb1", "/tmp", None).await;
        agent.client.push_response(vec![XaiEvent::Done]);

        let resource_block = ContentBlock::ResourceLink(ResourceLink::new(
            "context.md",
            "file:///workspace/context.md",
        ));

        let result = agent
            .prompt(PromptRequest::new("ntb1", vec![resource_block]))
            .await;

        assert!(result.is_ok(), "prompt with ResourceLink must not return an error");
        // User message (with ResourceLink text) is recorded in history.
        assert_eq!(agent.test_history_len("ntb1").await, 1);
    }

    // ── XAI_PROMPT_TIMEOUT_SECS=0 falls back to default ──────────────────────

    #[tokio::test]
    async fn prompt_timeout_zero_falls_back_to_default() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", "0") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS") };
        assert_eq!(
            agent.test_prompt_timeout(),
            Duration::from_secs(300),
            "XAI_PROMPT_TIMEOUT_SECS=0 must fall back to the 300s default"
        );
    }

    // ── build_input: non-assistant role treated as user ──────────────────────

    #[tokio::test]
    async fn build_input_non_assistant_role_treated_as_user() {
        // Any history message whose role is not "assistant" falls into the else
        // branch in build_full_history_input and becomes an InputItem with role
        // "user". Verify with an artificial "system" role history entry.
        let agent = make_agent();
        let history = vec![
            Message { role: "system".to_string(), content: Some("injected".to_string()) },
        ];
        agent.test_insert_session_with_history("bi1", "/tmp", history).await;
        agent.client.push_response(vec![XaiEvent::Done]);
        agent
            .prompt(PromptRequest::new("bi1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        // input: [history-item (role=user), new user item] — no system prompt set
        assert_eq!(calls[0].input[0].role, "user", "non-assistant role must be mapped to 'user'");
        assert_eq!(calls[0].input[0].content, "injected");
    }

    // ── with_deps: empty api_key sets global_api_key to None ─────────────────

    #[tokio::test]
    async fn with_deps_empty_api_key_sets_global_api_key_to_none() {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent = XaiAgent::with_deps(mock_notifier, "grok-3", "", mock_http);
        assert!(
            agent.global_api_key.is_none(),
            "empty api_key string must produce global_api_key = None"
        );
    }

    // ── build_input: system prompt with empty history ─────────────────────────

    #[tokio::test]
    async fn build_input_system_prompt_with_empty_history() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_SYSTEM_PROMPT", "Be concise.") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_SYSTEM_PROMPT") };

        agent.test_insert_session("sp_eh", "/tmp", None).await;
        mock_http.push_response(vec![XaiEvent::Done]);
        agent
            .prompt(PromptRequest::new("sp_eh", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        let calls = mock_http.calls.lock().unwrap();
        let input = &calls[0].input;
        assert_eq!(input.len(), 2, "system prompt + user item only (no history)");
        assert_eq!(input[0].role, "system");
        assert_eq!(input[0].content, "Be concise.");
        assert_eq!(input[1].role, "user");
    }

    // ── new_session stores cwd ────────────────────────────────────────────────

    #[tokio::test]
    async fn new_session_stores_cwd() {
        use agent_client_protocol::Agent as _;
        let agent = make_agent();
        let resp = agent
            .new_session(NewSessionRequest::new("/my/workspace"))
            .await
            .unwrap();
        let sid = resp.session_id.to_string();
        let stored_cwd = agent.test_session_cwd(&sid).await;
        assert_eq!(stored_cwd.as_deref(), Some("/my/workspace"));
    }

    // ── fork_session stores new cwd ───────────────────────────────────────────

    #[tokio::test]
    async fn fork_session_stores_new_cwd() {
        use agent_client_protocol::Agent as _;
        let agent = make_agent();
        let src = agent
            .new_session(NewSessionRequest::new("/src/cwd"))
            .await
            .unwrap();
        let src_id = src.session_id.to_string();

        let fork = agent
            .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork/cwd"))
            .await
            .unwrap();
        let fork_id = fork.session_id.to_string();

        let fork_cwd = agent.test_session_cwd(&fork_id).await;
        assert_eq!(fork_cwd.as_deref(), Some("/fork/cwd"), "fork must store its own cwd");

        let src_cwd = agent.test_session_cwd(&src_id).await;
        assert_eq!(src_cwd.as_deref(), Some("/src/cwd"), "source cwd must not be modified");
    }

    // ── XAI_MAX_TURNS invalid env var ─────────────────────────────────────────

    #[tokio::test]
    async fn xai_max_turns_invalid_env_var_falls_back_to_default() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_MAX_TURNS", "not_a_number") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MAX_TURNS") };
        assert_eq!(
            agent.max_turns,
            Some(10),
            "XAI_MAX_TURNS with non-numeric value must fall back to Some(10)"
        );
    }

    // ── XAI_MODELS whitespace-only entries fall back to defaults ─────────────

    #[tokio::test]
    async fn xai_models_whitespace_only_entries_falls_back_to_defaults() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_MODELS", "   ,   ") };
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let mock_notifier = Arc::new(MockSessionNotifier::new());
        let agent: TestAgent =
            XaiAgent::with_deps(Arc::clone(&mock_notifier), "grok-3", "key", Arc::clone(&mock_http));
        unsafe { std::env::remove_var("XAI_MODELS") };
        let ids: Vec<_> =
            agent.available_models.iter().map(|m| m.model_id.to_string()).collect();
        assert!(ids.contains(&"grok-3".to_string()), "default grok-3 must be present");
        assert!(ids.contains(&"grok-3-mini".to_string()), "default grok-3-mini must be present");
    }

    // ── trim_history ──────────────────────────────────────────────────────────

    fn msg(role: &str, content: &str) -> Message {
        Message { role: role.to_string(), content: Some(content.to_string()) }
    }

    fn roles(history: &[Message]) -> Vec<&str> {
        history.iter().map(|m| m.role.as_str()).collect()
    }

    #[test]
    fn trim_history_no_op_when_at_or_below_max() {
        let mut h = vec![msg("user", "a"), msg("assistant", "b")];
        trim_history(&mut h, 2);
        assert_eq!(h.len(), 2);
        trim_history(&mut h, 4);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn trim_history_removes_oldest_pair() {
        let mut h = vec![
            msg("user", "u1"),
            msg("assistant", "a1"),
            msg("user", "u2"),
            msg("assistant", "a2"),
            msg("user", "u3"),
            msg("assistant", "a3"),
        ];
        trim_history(&mut h, 4);
        assert_eq!(h.len(), 4);
        assert_eq!(h[0].content_str(), "u2");
    }

    #[test]
    fn trim_history_odd_max_removes_full_pair() {
        let mut h = vec![
            msg("user", "u1"),
            msg("assistant", "a1"),
            msg("user", "u2"),
            msg("assistant", "a2"),
        ];
        trim_history(&mut h, 3);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].content_str(), "u2");
        assert_eq!(h[1].content_str(), "a2");
    }

    #[test]
    fn trim_history_orphaned_user_removed_individually() {
        let mut h = vec![
            msg("user", "orphan"),
            msg("user", "new"),
            msg("assistant", "reply"),
        ];
        trim_history(&mut h, 2);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].content_str(), "new");
        assert_eq!(h[1].content_str(), "reply");
    }

    #[test]
    fn trim_history_multiple_orphans_then_pair() {
        let mut h = vec![
            msg("user", "orphan1"),
            msg("user", "orphan2"),
            msg("user", "new"),
            msg("assistant", "reply"),
        ];
        trim_history(&mut h, 2);
        assert_eq!(roles(&h), vec!["user", "assistant"]);
        assert_eq!(h[0].content_str(), "new");
    }

    #[test]
    fn trim_history_stops_on_unexpected_leading_role() {
        let mut h = vec![msg("assistant", "a"), msg("user", "u")];
        trim_history(&mut h, 1);
        assert_eq!(h.len(), 2, "should not trim malformed leading assistant");
    }

    #[test]
    fn trim_history_empty_is_noop() {
        let mut h: Vec<Message> = vec![];
        trim_history(&mut h, 0);
        assert!(h.is_empty());
    }

    // ── enabled_tools: passed to chat_stream ─────────────────────────────────

    #[tokio::test]
    async fn prompt_sends_enabled_tools_to_chat_stream() {
        // After enabling web_search, the next prompt must pass it in the `tools`
        // parameter of chat_stream — the toggle must reach the HTTP layer.
        let agent = make_agent();
        agent.test_insert_session("tl1", "/tmp", None).await;
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("tl1", "web_search", "on"))
            .await
            .unwrap();

        agent.client.push_response(vec![XaiEvent::Done]);
        agent.prompt(PromptRequest::new("tl1", vec![ContentBlock::from("hi")])).await.unwrap();

        let calls = agent.client.calls.lock().unwrap();
        assert_eq!(
            calls.last().unwrap().tools,
            vec!["web_search".to_string()],
            "enabled web_search must be forwarded to chat_stream"
        );
    }

    // ── set_session_config_option: x_search toggle ────────────────────────────

    #[tokio::test]
    async fn set_session_config_option_toggles_x_search() {
        let agent = make_agent();
        agent.test_insert_session("xs1", "/tmp", None).await;

        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("xs1", "x_search", "on"))
            .await
            .unwrap();
        assert!(
            agent.test_session_enabled_tools("xs1").await.contains(&"x_search".to_string()),
            "x_search must be in enabled_tools after turning on"
        );

        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("xs1", "x_search", "off"))
            .await
            .unwrap();
        assert!(
            !agent.test_session_enabled_tools("xs1").await.contains(&"x_search".to_string()),
            "x_search must be removed from enabled_tools after turning off"
        );
    }

    // ── fork: inherits enabled_tools ─────────────────────────────────────────

    #[tokio::test]
    async fn fork_session_inherits_enabled_tools() {
        let agent = make_agent();
        agent.test_insert_session("ft_src", "/tmp", None).await;
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "ft_src",
                "web_search",
                "on",
            ))
            .await
            .unwrap();

        let resp = agent.fork_session(ForkSessionRequest::new("ft_src", "/fork_t")).await.unwrap();
        let fork_id = resp.session_id.to_string();

        assert!(
            agent.test_session_enabled_tools(&fork_id).await.contains(&"web_search".to_string()),
            "fork must inherit enabled_tools from source session"
        );
    }

    // ── prompt: stale-ID retry ────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_stale_id_retry_on_non_4xx_error() {
        // When `previous_response_id` is set and the first call returns a non-4xx
        // error, the agent must retry once with the full history and no ID.
        let agent = make_agent();
        agent
            .test_insert_session_with_response_id(
                "stale1",
                "/tmp",
                None,
                Some("stale-id".to_string()),
            )
            .await;
        // First call → non-4xx error triggers stale-ID retry.
        agent.client.push_response(vec![XaiEvent::Error { message: "upstream timeout".to_string() }]);
        // Second call → success.
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("stale1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "stale-ID retry must cause exactly 2 HTTP calls");
        assert_eq!(
            calls[0].previous_response_id.as_deref(),
            Some("stale-id"),
            "first call must use the cached previous_response_id"
        );
        assert_eq!(
            calls[1].previous_response_id,
            None,
            "retry call must not use previous_response_id"
        );
    }

    // ── prompt: EmbeddedResource content blocks ───────────────────────────────

    #[tokio::test]
    async fn prompt_embedded_text_resource_content_included_in_input() {
        use agent_client_protocol::{EmbeddedResource, EmbeddedResourceResource, TextResourceContents};

        let agent = make_agent();
        agent.test_insert_session("emb1", "/tmp", None).await;
        agent.client.push_response(vec![XaiEvent::Done]);

        let block = ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                "embedded text content",
                "file:///doc.md",
            )),
        ));

        agent.prompt(PromptRequest::new("emb1", vec![block])).await.unwrap();

        let calls = agent.client.calls.lock().unwrap();
        let user_item = calls.last().unwrap().input.last().unwrap();
        assert!(
            user_item.content.contains("embedded text content"),
            "EmbeddedResource text must appear in the chat_stream input: {}",
            user_item.content
        );
    }

    #[tokio::test]
    async fn prompt_embedded_binary_resource_noted_in_input() {
        use agent_client_protocol::{BlobResourceContents, EmbeddedResource, EmbeddedResourceResource};

        let agent = make_agent();
        agent.test_insert_session("emb2", "/tmp", None).await;
        agent.client.push_response(vec![XaiEvent::Done]);

        let block = ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::BlobResourceContents(
                BlobResourceContents::new("base64data==", "file:///image.png")
                    .mime_type("image/png"),
            ),
        ));

        agent.prompt(PromptRequest::new("emb2", vec![block])).await.unwrap();

        let calls = agent.client.calls.lock().unwrap();
        let user_item = calls.last().unwrap().input.last().unwrap();
        assert!(
            user_item.content.contains("[Binary resource:"),
            "EmbeddedResource binary must produce a '[Binary resource: ...]' note: {}",
            user_item.content
        );
        assert!(
            user_item.content.contains("image/png"),
            "binary resource note must include MIME type: {}",
            user_item.content
        );
    }

    // ── prompt: Usage event carries correct token counts ─────────────────────

    #[tokio::test]
    async fn prompt_usage_event_emits_notification_with_correct_token_count() {
        let agent = make_agent();
        agent.test_insert_session("usg1", "/tmp", None).await;
        agent.client.push_response(vec![
            XaiEvent::Usage { prompt_tokens: 42, completion_tokens: 10 },
            XaiEvent::Done,
        ]);

        agent.prompt(PromptRequest::new("usg1", vec![ContentBlock::from("hi")])).await.unwrap();

        let notifs = agent.notifier.notifications.lock().unwrap();
        let usage_notif = notifs.iter().find(|n| {
            matches!(&n.update, SessionUpdate::UsageUpdate(_))
        });
        let update = match usage_notif.expect("UsageUpdate notification must be emitted").update {
            SessionUpdate::UsageUpdate(ref u) => u,
            _ => unreachable!(),
        };
        assert_eq!(update.used, 42, "UsageUpdate.used must equal prompt_tokens");
        assert_eq!(update.size, 10, "UsageUpdate.size must equal completion_tokens");
    }

    // ── FinishReason::Other is treated as end-of-turn ────────────────────────

    #[tokio::test]
    async fn prompt_finish_reason_other_completes_normally() {
        // An unknown/future finish reason must not panic — treat as EndTurn.
        let agent = make_agent();
        agent.test_insert_session("fr1", "/tmp", None).await;
        agent.client.push_response(vec![
            XaiEvent::TextDelta { text: "hi".to_string() },
            XaiEvent::Finished {
                reason: FinishReason::Other("rate_limited".to_string()),
                incomplete_reason: None,
            },
            XaiEvent::Done,
        ]);

        let result = agent
            .prompt(PromptRequest::new("fr1", vec![ContentBlock::from("hello")]))
            .await
            .unwrap();
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        // Text before the unknown Finished is preserved in history.
        assert_eq!(agent.test_history_len("fr1").await, 2);
    }

    // ── continuation: max_output_tokens sends empty input ────────────────────

    #[tokio::test]
    async fn prompt_incomplete_max_output_tokens_continuation_sends_empty_input() {
        // When Incomplete reason is "max_output_tokens", the continuation must send
        // an empty input array so xAI resumes from its cached context without
        // re-injecting the user message.
        let agent = make_agent();
        agent.test_insert_session("cnt1", "/tmp", None).await;
        // First: Incomplete with max_output_tokens.
        agent.client.push_response(vec![
            XaiEvent::ResponseId { id: "resp-1".to_string() },
            XaiEvent::Finished {
                reason: FinishReason::Incomplete,
                incomplete_reason: Some("max_output_tokens".to_string()),
            },
        ]);
        // Second: success.
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("cnt1", vec![ContentBlock::from("long query")]))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "max_output_tokens continuation must make 2 calls");
        assert!(
            calls[1].input.is_empty(),
            "max_output_tokens continuation must send empty input, got {} items",
            calls[1].input.len()
        );
    }

    #[tokio::test]
    async fn prompt_incomplete_other_reason_continuation_resends_user_message() {
        // Non-max_output_tokens Incomplete reason re-sends the user message in the
        // continuation so xAI knows what task to continue.
        let agent = make_agent();
        agent.test_insert_session("cnt2", "/tmp", None).await;
        agent.client.push_response(vec![
            XaiEvent::ResponseId { id: "resp-2".to_string() },
            XaiEvent::Finished {
                reason: FinishReason::Incomplete,
                incomplete_reason: Some("max_turns".to_string()),
            },
        ]);
        agent.client.push_response(vec![XaiEvent::Done]);

        agent
            .prompt(PromptRequest::new("cnt2", vec![ContentBlock::from("my query")]))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[1].input.len(),
            1,
            "non-max_output_tokens continuation must re-send the user message"
        );
        assert_eq!(calls[1].input[0].content, "my query");
    }

    // ── continuation: MAX_CONTINUATIONS limit ────────────────────────────────

    #[tokio::test]
    async fn prompt_max_continuations_exceeded_returns_cancelled() {
        // After MAX_CONTINUATIONS (5) continuation attempts the agent must stop
        // and return StopReason::Cancelled rather than looping forever.
        let agent = make_agent();
        agent.test_insert_session("cnt3", "/tmp", None).await;

        // Push 6 Incomplete responses — one initial + 5 continuations exhaust the
        // limit; the 6th inner loop fires the ≥ MAX_CONTINUATIONS guard.
        for i in 0..6 {
            agent.client.push_response(vec![
                XaiEvent::ResponseId { id: format!("r{i}") },
                XaiEvent::Finished {
                    reason: FinishReason::Incomplete,
                    incomplete_reason: None,
                },
            ]);
        }

        let result = agent
            .prompt(PromptRequest::new("cnt3", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();
        assert_eq!(
            result.stop_reason,
            StopReason::Cancelled,
            "exceeding MAX_CONTINUATIONS must return StopReason::Cancelled"
        );
        let calls = agent.client.calls.lock().unwrap();
        assert_eq!(calls.len(), 6, "must make exactly 6 HTTP calls before stopping");
    }

    // ── stale-ID retry: partial text accumulated before error is discarded ────

    #[tokio::test]
    async fn prompt_stale_id_retry_discards_partial_text_from_failed_attempt() {
        // When the first call (with stale previous_response_id) yields some text
        // then errors, the accumulated text must be cleared before retrying.
        // The final history should contain only the text from the successful retry.
        let agent = make_agent();
        agent
            .test_insert_session_with_response_id(
                "stale2",
                "/tmp",
                None,
                Some("stale-id".to_string()),
            )
            .await;

        // First attempt: partial text then non-4xx error.
        agent.client.push_response(vec![
            XaiEvent::TextDelta { text: "stale partial ".to_string() },
            XaiEvent::Error { message: "connection reset".to_string() },
        ]);
        // Retry: clean response.
        agent.client.push_response(vec![
            XaiEvent::TextDelta { text: "clean answer".to_string() },
            XaiEvent::Done,
        ]);

        agent
            .prompt(PromptRequest::new("stale2", vec![ContentBlock::from("question")]))
            .await
            .unwrap();

        // History must contain only the clean retry response.
        let history = agent.test_session_history("stale2").await;
        assert_eq!(history.len(), 2, "user + assistant = 2 entries");
        let assistant = &history[1];
        assert_eq!(
            assistant.content_str(),
            "clean answer",
            "stale partial text must be discarded; only retry text kept"
        );
    }

    // ── multi-tool toggle idempotency ─────────────────────────────────────────

    #[tokio::test]
    async fn set_session_config_option_multi_tool_toggle_idempotency() {
        // Enable both tools, then disable one — the other must remain enabled.
        let agent = make_agent();
        agent.test_insert_session("mt1", "/tmp", None).await;

        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("mt1", "web_search", "on"))
            .await
            .unwrap();
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("mt1", "x_search", "on"))
            .await
            .unwrap();
        // Enable web_search again (idempotent — must not duplicate).
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("mt1", "web_search", "on"))
            .await
            .unwrap();
        // Disable web_search — x_search must remain.
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "mt1",
                "web_search",
                "off",
            ))
            .await
            .unwrap();

        let enabled = agent.test_session_enabled_tools("mt1").await;
        assert!(
            !enabled.contains(&"web_search".to_string()),
            "web_search must be disabled"
        );
        assert!(enabled.contains(&"x_search".to_string()), "x_search must remain enabled");
        assert_eq!(enabled.len(), 1, "only x_search; idempotent re-enable must not duplicate");
    }

    // ── 4xx error does not trigger stale-ID retry ─────────────────────────────

    #[tokio::test]
    async fn prompt_4xx_error_does_not_trigger_stale_id_retry() {
        // A 4xx error with a stale previous_response_id must NOT retry — only
        // non-4xx errors trigger the stale-ID recovery path.
        let agent = make_agent();
        agent
            .test_insert_session_with_response_id(
                "no_retry1",
                "/tmp",
                None,
                Some("prev-id".to_string()),
            )
            .await;
        // Queue only one response; retry would panic (no second response queued).
        agent.client.push_response(vec![XaiEvent::Error {
            message: "xAI API error 429: Too Many Requests".to_string(),
        }]);

        agent
            .prompt(PromptRequest::new("no_retry1", vec![ContentBlock::from("hi")]))
            .await
            .unwrap();

        let calls = agent.client.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "4xx error must not trigger stale-ID retry");
    }

    // ── parse_tool_arguments ──────────────────────────────────────────────────

    #[test]
    fn parse_tool_arguments_valid_json_returns_object() {
        let val = parse_tool_arguments(r#"{"q":"test","limit":5}"#);
        assert!(val.is_object(), "expected object, got {val:?}");
        assert_eq!(val["q"], "test");
        assert_eq!(val["limit"], 5);
    }

    #[test]
    fn parse_tool_arguments_invalid_json_falls_back_to_string() {
        let val = parse_tool_arguments("not-json");
        assert_eq!(val, serde_json::Value::String("not-json".to_string()));
    }

    #[test]
    fn parse_tool_arguments_empty_string_falls_back_to_string() {
        let val = parse_tool_arguments("");
        assert_eq!(val, serde_json::Value::String(String::new()));
    }
}
