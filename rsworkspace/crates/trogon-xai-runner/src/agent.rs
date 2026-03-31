use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{
    AgentCapabilities, AuthEnvVar, AuthMethod, AuthMethodAgent, AuthMethodEnvVar, PromptCapabilities,
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ContentChunk, EmbeddedResourceResource, Error, ErrorCode,
    ForkSessionRequest, ForkSessionResponse, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, ModelInfo,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, ProtocolVersion,
    ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionCloseCapabilities,
    SessionConfigOption, SessionConfigOptionValue, SessionConfigSelectOption,
    SessionForkCapabilities, SessionId, SessionInfo, SessionListCapabilities, SessionMode,
    SessionModeState, SessionModelState, SessionNotification, SessionResumeCapabilities,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse,
    StopReason, ToolCall, ToolCallStatus, ToolKind, UsageUpdate,
};
use agent_client_protocol::Client as _;
use async_trait::async_trait;
use futures_util::StreamExt as _;
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::client::{Message, XaiClient, XaiEvent};
use crate::session_store::{KvSessionStore, SessionStore, XaiSessionData};
#[cfg(feature = "test-helpers")]
use crate::session_store::MemorySessionStore;

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

fn not_found(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::ResourceNotFound.into(), msg.into())
}

fn auth_required(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::AuthRequired.into(), msg.into())
}

fn invalid_params(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidParams.into(), msg.into())
}

fn store_error(e: impl std::fmt::Display) -> Error {
    internal_error(format!("session store error: {e}"))
}

/// ACP Agent implementation backed by xAI's Grok API (OpenAI-compatible REST).
///
/// Each `XaiAgent` manages multiple sessions, each holding its own conversation
/// history persisted in NATS KV. Prompt calls stream chat completions from
/// `api.x.ai` and forward text chunks to the ACP client via NATS
/// `SessionNotification`s.
pub struct XaiAgent {
    nats: async_nats::Client,
    acp_prefix: AcpPrefix,
    client: Arc<XaiClient>,
    session_store: Box<dyn SessionStore>,
    /// In-memory cancel channels — one per active prompt, keyed by session id.
    cancel_channels: Arc<Mutex<HashMap<String, Arc<Mutex<Option<oneshot::Sender<()>>>>>>>,
    default_model: String,
    /// Per-chunk inactivity timeout for streaming responses.
    ///
    /// Applied to each `stream.next()` call — fires if no chunk arrives within
    /// this duration. A slow but continuously streaming response will NOT be
    /// cut off. Configured via `XAI_PROMPT_TIMEOUT_SECS` (default: 300s).
    prompt_timeout: Duration,
    available_models: Vec<ModelInfo>,
    /// Server-wide fallback key (from `XAI_API_KEY` env var at startup).
    global_api_key: Option<String>,
    /// Holds the key extracted from the last `authenticate` call until the
    /// next `new_session` picks it up.
    ///
    /// FIXME: single-slot — race if two clients authenticate concurrently before
    /// calling `new_session`. The second `authenticate` overwrites the first key.
    /// Fixing this properly requires correlating authenticate→new_session per
    /// connection, which needs framework support.
    pending_api_key: Arc<Mutex<Option<String>>>,
    /// Optional system prompt injected at the start of every conversation.
    /// Read from `XAI_SYSTEM_PROMPT` at construction time.
    system_prompt: Option<String>,
    /// Maximum number of messages kept in history (user + assistant interleaved).
    /// Oldest messages are trimmed in pairs to preserve user/assistant ordering.
    /// Configured via `XAI_MAX_HISTORY_MESSAGES` (default: 20 = 10 exchanges).
    ///
    /// Note: truncation is by message count, not tokens. With `search_mode` active,
    /// messages can be long; lower this value if context window errors occur.
    max_history_messages: usize,
}

impl XaiAgent {
    /// Create a new `XaiAgent` backed by NATS KV for session persistence.
    ///
    /// The KV bucket name is read from `XAI_SESSION_BUCKET` (default: `XAI_SESSIONS`).
    ///
    /// Other environment variables:
    /// - `XAI_PROMPT_TIMEOUT_SECS` — per-chunk inactivity timeout in seconds (default: 300; 0 = default)
    /// - `XAI_MAX_HISTORY_MESSAGES` — max messages kept in history (default: 40; 0 = default)
    /// - `XAI_MODELS` — comma-separated `id:label` pairs
    /// - `XAI_BASE_URL` — override the xAI API base URL
    pub async fn new(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let bucket = std::env::var("XAI_SESSION_BUCKET")
            .unwrap_or_else(|_| "XAI_SESSIONS".to_string());
        let session_ttl = std::env::var("XAI_SESSION_TTL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(7 * 24 * 3600));
        let js = async_nats::jetstream::new(nats.clone());
        let store = KvSessionStore::open(js, bucket, session_ttl).await?;
        Ok(Self::new_with_store(nats, acp_prefix, default_model, api_key, Box::new(store)))
    }

    /// Internal constructor with an injected session store.
    fn new_with_store(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        session_store: Box<dyn SessionStore>,
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
                    .filter_map(|entry| {
                        match entry.split_once(':') {
                            Some((id, label)) => {
                                Some(ModelInfo::new(id.trim().to_string(), label.trim().to_string()))
                            }
                            None => {
                                warn!(entry, "XAI_MODELS: skipping malformed entry (expected 'id:label')");
                                None
                            }
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

        if !available_models.iter().any(|m| m.model_id.0.as_ref() == default_model.as_str()) {
            warn!(model = %default_model, "default model not in available list; adding it");
            available_models.push(ModelInfo::new(default_model.clone(), default_model.clone()));
        }

        let system_prompt = std::env::var("XAI_SYSTEM_PROMPT").ok()
            .filter(|s| !s.is_empty());

        let max_history_messages = std::env::var("XAI_MAX_HISTORY_MESSAGES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(20);

        Self {
            nats,
            acp_prefix,
            client: Arc::new(XaiClient::new()),
            session_store,
            cancel_channels: Arc::new(Mutex::new(HashMap::new())),
            default_model,
            prompt_timeout,
            available_models,
            global_api_key,
            pending_api_key: Arc::new(Mutex::new(None)),
            system_prompt,
            max_history_messages,
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

    /// Build the `search_mode` `SessionConfigOption` with the given current value.
    fn search_mode_config_option(current: &str) -> SessionConfigOption {
        SessionConfigOption::select(
            "search_mode",
            "Web Search",
            current.to_string(),
            vec![
                SessionConfigSelectOption::new("off", "Off"),
                SessionConfigSelectOption::new("auto", "Auto"),
                SessionConfigSelectOption::new("on", "On"),
            ],
        )
        .description("Enable xAI server-side web and X search (no round-trip required)")
    }

}

#[async_trait(?Send)]
impl agent_client_protocol::Agent for XaiAgent {
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
                    .prompt_capabilities(
                        PromptCapabilities::new().embedded_context(true),
                    )
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
        let method = req.method_id.0.as_ref();
        match method {
            "xai-api-key" => {
                let key = req
                    .meta
                    .as_ref()
                    .and_then(|m| m.get("XAI_API_KEY"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        auth_required(
                            "authenticate: XAI_API_KEY missing from _meta for method 'xai-api-key'",
                        )
                    })?;
                info!("xai: user authenticated with their own API key");
                *self.pending_api_key.lock().await = Some(key);
            }
            "agent" => {
                if self.global_api_key.is_none() {
                    return Err(auth_required(
                        "authenticate: no server API key configured; use method 'xai-api-key' instead",
                    ));
                }
                info!("xai: client authenticated using server key");
            }
            other => {
                return Err(invalid_params(format!("authenticate: unknown method '{other}'")));
            }
        }
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(
        &self,
        req: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        let cwd = req.cwd.to_string_lossy().into_owned();
        let session_id = Uuid::new_v4().to_string();

        let api_key = self.pending_api_key.lock().await.take()
            .or_else(|| self.global_api_key.clone());

        self.session_store.put(&session_id, &XaiSessionData {
            cwd,
            model: None,
            history: Vec::new(),
            api_key,
            system_prompt: self.system_prompt.clone(),
            search_mode: None,
        }).await.map_err(store_error)?;

        info!(session_id, "xai: new session");
        Ok(NewSessionResponse::new(SessionId::from(session_id))
            .modes(self.session_mode_state())
            .models(self.session_model_state(None))
            .config_options(vec![Self::search_mode_config_option("off")]))
    }

    async fn load_session(
        &self,
        req: LoadSessionRequest,
    ) -> agent_client_protocol::Result<LoadSessionResponse> {
        let session_id = req.session_id.to_string();
        let session = self.session_store.get(&session_id).await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
        let model = session.model.as_deref();
        let search_mode = session.search_mode.as_deref().unwrap_or("off");
        Ok(LoadSessionResponse::new()
            .modes(self.session_mode_state())
            .models(self.session_model_state(model))
            .config_options(vec![Self::search_mode_config_option(search_mode)]))
    }

    async fn resume_session(
        &self,
        req: ResumeSessionRequest,
    ) -> agent_client_protocol::Result<ResumeSessionResponse> {
        let session_id = req.session_id.to_string();
        self.session_store.get(&session_id).await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
        Ok(ResumeSessionResponse::new())
    }

    async fn fork_session(
        &self,
        req: ForkSessionRequest,
    ) -> agent_client_protocol::Result<ForkSessionResponse> {
        let source_id = req.session_id.to_string();
        let cwd = req.cwd.to_string_lossy().into_owned();

        let source = self.session_store.get(&source_id).await
            .ok_or_else(|| not_found(format!("session {source_id} not found")))?;

        let new_session_id = Uuid::new_v4().to_string();
        let inherited_model = source.model.clone();
        let inherited_search_mode = source.search_mode.clone();
        self.session_store.put(&new_session_id, &XaiSessionData {
            cwd,
            model: inherited_model.clone(),
            history: source.history,
            api_key: source.api_key,
            system_prompt: source.system_prompt,
            search_mode: inherited_search_mode.clone(),
        }).await.map_err(store_error)?;

        let search_mode_str = inherited_search_mode.as_deref().unwrap_or("off");
        Ok(ForkSessionResponse::new(new_session_id)
            .modes(self.session_mode_state())
            .models(self.session_model_state(inherited_model.as_deref()))
            .config_options(vec![Self::search_mode_config_option(search_mode_str)]))
    }

    async fn close_session(
        &self,
        req: CloseSessionRequest,
    ) -> agent_client_protocol::Result<CloseSessionResponse> {
        let session_id = req.session_id.to_string();
        self.session_store.delete(&session_id).await;
        self.cancel_channels.lock().await.remove(&session_id);
        info!(session_id, "xai: session closed");
        Ok(CloseSessionResponse::new())
    }

    async fn list_sessions(
        &self,
        _req: ListSessionsRequest,
    ) -> agent_client_protocol::Result<ListSessionsResponse> {
        let list = self.session_store.list().await
            .into_iter()
            .map(|(id, cwd)| SessionInfo::new(id, cwd))
            .collect();
        Ok(ListSessionsResponse::new(list))
    }

    async fn set_session_mode(
        &self,
        req: SetSessionModeRequest,
    ) -> agent_client_protocol::Result<SetSessionModeResponse> {
        let session_id = req.session_id.to_string();
        self.session_store.get(&session_id).await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
        let mode_id = req.mode_id.to_string();
        if mode_id != "default" {
            return Err(invalid_params(format!("unknown mode: {mode_id}")));
        }
        Ok(SetSessionModeResponse::new())
    }

    async fn set_session_model(
        &self,
        req: SetSessionModelRequest,
    ) -> agent_client_protocol::Result<SetSessionModelResponse> {
        let session_id = req.session_id.to_string();
        let model_id = req.model_id.to_string();

        if !self.available_models.iter().any(|m| m.model_id.0.as_ref() == model_id) {
            return Err(invalid_params(format!("unknown model: {model_id}")));
        }

        let mut session = self.session_store.get(&session_id).await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
        session.model = Some(model_id.clone());
        self.session_store.put(&session_id, &session).await.map_err(store_error)?;

        info!(session_id, model = %model_id, "xai: set_session_model");
        Ok(SetSessionModelResponse::new())
    }

    async fn set_session_config_option(
        &self,
        req: SetSessionConfigOptionRequest,
    ) -> agent_client_protocol::Result<SetSessionConfigOptionResponse> {
        let config_id = req.config_id.to_string();
        match config_id.as_str() {
            "search_mode" => {
                let SessionConfigOptionValue::ValueId { value } = req.value else {
                    return Err(invalid_params("search_mode requires a string value (off/auto/on)"));
                };
                let mode = value.to_string();
                if !["off", "auto", "on"].contains(&mode.as_str()) {
                    return Err(invalid_params(format!(
                        "unknown search_mode '{mode}'; expected: off, auto, on"
                    )));
                }
                let session_id = req.session_id.to_string();
                let mut session = self.session_store.get(&session_id).await
                    .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
                session.search_mode = if mode == "off" { None } else { Some(mode.clone()) };
                self.session_store.put(&session_id, &session).await.map_err(store_error)?;
                info!(session_id, mode, "xai: search_mode updated");
                Ok(SetSessionConfigOptionResponse::new(vec![Self::search_mode_config_option(&mode)]))
            }
            other => {
                warn!(config_id = %other, "xai: set_session_config_option called for unknown option — ignored");
                // Per ACP spec, return the current state of all known options
                // even when the requested config_id is unknown — but the session
                // must still exist.
                let session_id = req.session_id.to_string();
                let session = self.session_store.get(&session_id).await
                    .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
                let search_mode = session.search_mode.as_deref().unwrap_or("off");
                Ok(SetSessionConfigOptionResponse::new(vec![Self::search_mode_config_option(search_mode)]))
            }
        }
    }

    async fn prompt(
        &self,
        req: PromptRequest,
    ) -> agent_client_protocol::Result<PromptResponse> {
        let session_id = req.session_id.to_string();
        let nats_client = self.make_nats_client(&req.session_id)?;

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

        // Read session state.
        let session = self.session_store.get(&session_id).await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;

        let api_key = session.api_key.clone().ok_or_else(|| {
            auth_required(
                "no API key for this session: authenticate with method 'xai-api-key' first",
            )
        })?;

        let model = session.model.as_deref().unwrap_or(&self.default_model).to_string();
        let search_mode = session.search_mode.clone();

        // Install a cancel channel, releasing the cancel_channels lock before streaming.
        let cancel_arc = {
            let mut channels = self.cancel_channels.lock().await;
            channels
                .entry(session_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        cancel_arc.lock().await.replace(cancel_tx);

        // Persist the user message BEFORE calling xAI so that a crash between
        // the request and the response cannot silently discard the user's input.
        {
            let mut snapshot = session.clone();
            snapshot.history.push(Message {
                role: "user".to_string(),
                content: user_input.clone(),
            });
            self.session_store.put(&session_id, &snapshot).await.map_err(store_error)?;
        }

        // Build messages: optional system prompt + history + new user turn.
        let mut messages: Vec<Message> = Vec::new();
        if let Some(sp) = &session.system_prompt {
            messages.push(Message { role: "system".to_string(), content: sp.clone() });
        }
        messages.extend(session.history.clone());
        messages.push(Message { role: "user".to_string(), content: user_input.clone() });

        let client = Arc::clone(&self.client);
        let mut stream = client
            .chat_stream(&model, &messages, &api_key, search_mode.as_deref())
            .await;

        let mut assistant_text = String::new();
        let mut canceled = false;

        let stop_reason = 'turn: loop {
            let event = tokio::select! {
                biased;
                _ = &mut cancel_rx => {
                    info!(session_id, "xai: prompt canceled");
                    canceled = true;
                    break 'turn StopReason::Cancelled;
                }
                maybe = tokio::time::timeout(self.prompt_timeout, stream.next()) => {
                    match maybe {
                        Err(_elapsed) => {
                            warn!(session_id, "xai: prompt timed out");
                            break 'turn StopReason::EndTurn;
                        }
                        Ok(Some(e)) => e,
                        Ok(None) => break 'turn StopReason::EndTurn,
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
                    if let Err(e) = nats_client.session_notification(notif).await {
                        warn!(session_id, error = %e, "xai: failed to send text notification");
                    }
                }
                XaiEvent::ToolCallStart { id, name, arguments } => {
                    info!(session_id, tool_id = %id, tool_name = %name, "xai: tool call requested");
                    // Notify the ACP client that the model requested a tool call.
                    // Status is Pending — the runner does not execute tools; the
                    // client is responsible for running the tool and returning the
                    // result as the next prompt turn.
                    //
                    // LIMITATION: the tool call round-trip is not fully supported.
                    // xAI expects tool results as {role:"tool", tool_call_id, content},
                    // but Message only stores {role, content}. The client's tool
                    // result will arrive as a plain user message, which breaks the
                    // structured tool call cycle. For web search, use search_mode
                    // instead — xAI handles it server-side without tool calls.
                    let tool_call = ToolCall::new(id.clone(), name.clone())
                        .status(ToolCallStatus::Pending)
                        .kind(ToolKind::Other)
                        .raw_input(parse_tool_arguments(&arguments));
                    let notif = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::ToolCall(tool_call),
                    );
                    if let Err(e) = nats_client.session_notification(notif).await {
                        warn!(session_id, error = %e, "xai: failed to send tool call notification");
                    }
                }
                XaiEvent::Usage { prompt_tokens, completion_tokens } => {
                    let total = prompt_tokens.saturating_add(completion_tokens);
                    info!(session_id, prompt_tokens, completion_tokens, "xai: token usage");
                    let notif = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::UsageUpdate(UsageUpdate::new(total, 0)),
                    );
                    if let Err(e) = nats_client.session_notification(notif).await {
                        warn!(session_id, error = %e, "xai: failed to send usage update");
                    }
                }
                XaiEvent::Done => break 'turn StopReason::EndTurn,
                XaiEvent::Error { message } => {
                    tracing::error!(session_id, error = %message, "xai: stream error");
                    return Err(internal_error(message));
                }
            }
        };

        if canceled {
            // Cancel compensation: the user message was persisted before the xAI
            // call to survive crashes, but if the prompt was canceled before any
            // response was produced we remove it so the history stays clean.
            // This is best-effort — a crash here leaves an orphaned user message,
            // which is acceptable (the message was sent; the turn was just cut short).
            if let Some(mut current) = self.session_store.get(&session_id).await {
                if current.history.last().map(|m| m.role == "user" && m.content == user_input)
                    == Some(true)
                {
                    current.history.pop();
                    let _ = self.session_store.put(&session_id, &current).await;
                }
            }
        } else if !assistant_text.is_empty() {
            // Re-read to preserve any concurrent model/config changes, then append
            // the assistant reply. The user message is already in the store.
            if let Some(mut current) = self.session_store.get(&session_id).await {
                current.history.push(Message {
                    role: "assistant".to_string(),
                    content: assistant_text,
                });

                // Trim oldest messages to stay within the configured limit.
                // Round up to even so we always drop complete user/assistant pairs.
                if current.history.len() > self.max_history_messages {
                    let excess = current.history.len() - self.max_history_messages;
                    let trim = (excess + 1) & !1;
                    current.history.drain(..trim);
                }

                self.session_store.put(&session_id, &current).await.map_err(store_error)?;
            }
        }

        Ok(PromptResponse::new(stop_reason))
    }

    async fn cancel(
        &self,
        req: CancelNotification,
    ) -> agent_client_protocol::Result<()> {
        let session_id = req.session_id.to_string();
        if let Some(cancel_arc) = self.cancel_channels.lock().await.get(&session_id).cloned() {
            if let Some(tx) = cancel_arc.lock().await.take() {
                let _ = tx.send(());
            }
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

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

#[cfg(feature = "test-helpers")]
impl XaiAgent {
    /// Creates an `XaiAgent` backed by a specific NATS KV bucket. For tests only.
    pub async fn new_with_kv_bucket(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        bucket: impl Into<String>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let js = async_nats::jetstream::new(nats.clone());
        let store = KvSessionStore::open(js, bucket, Duration::from_secs(7 * 24 * 3600)).await?;
        Ok(Self::new_with_store(nats, acp_prefix, default_model, api_key, Box::new(store)))
    }

    /// Creates an `XaiAgent` backed by an in-memory session store. For tests only.
    pub fn new_in_memory(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self::new_with_store(
            nats,
            acp_prefix,
            default_model,
            api_key,
            Box::new(MemorySessionStore::new()),
        )
    }

    pub async fn test_session_history(&self, id: &str) -> Vec<Message> {
        self.session_store.get(id).await.map(|s| s.history).unwrap_or_default()
    }

    pub async fn test_session_model(&self, id: &str) -> Option<String> {
        self.session_store.get(id).await.and_then(|s| s.model)
    }

    pub fn test_prompt_timeout(&self) -> Duration {
        self.prompt_timeout
    }

    pub fn test_max_history_messages(&self) -> usize {
        self.max_history_messages
    }
}
