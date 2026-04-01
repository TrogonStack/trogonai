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

use crate::client::{FinishReason, InputItem, Message, XaiClient, XaiEvent};
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

/// xAI server-side tools that can be toggled per session.
///
/// Each entry is `(tool_id, display_label)`. The `tool_id` is sent verbatim in
/// the Responses API `tools` array; the label is shown in the ACP UI.
const AVAILABLE_TOOLS: &[(&str, &str)] = &[
    ("web_search", "Web Search"),
    ("x_search", "X Search"),
    ("code_interpreter", "Code Interpreter"),
];

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
    /// Maximum agentic tool-call turns per prompt.
    ///
    /// Passed as `max_turns` in the Responses API request when tools are enabled.
    /// The xAI server caps its internal tool-calling loop at this value.
    /// Configured via `XAI_MAX_TURNS` (default: 10; 0 = use server default).
    max_turns: Option<u32>,
}

impl XaiAgent {
    /// Create a new `XaiAgent` backed by NATS KV for session persistence.
    ///
    /// The KV bucket name is read from `XAI_SESSION_BUCKET` (default: `XAI_SESSIONS`).
    ///
    /// Other environment variables:
    /// - `XAI_PROMPT_TIMEOUT_SECS` — per-chunk inactivity timeout in seconds (default: 300; 0 = default)
    /// - `XAI_MAX_HISTORY_MESSAGES` — max messages kept in history (default: 20; 0 = default)
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

        let max_turns = std::env::var("XAI_MAX_TURNS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&n| n > 0)
            .or(Some(10));

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
                if key.is_empty() {
                    return Err(auth_required(
                        "authenticate: XAI_API_KEY must not be empty",
                    ));
                }
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
            enabled_tools: Vec::new(),
            last_response_id: None,
        }).await.map_err(store_error)?;

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
        let session = self.session_store.get(&session_id).await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
        let model = session.model.as_deref();
        Ok(LoadSessionResponse::new()
            .modes(self.session_mode_state())
            .models(self.session_model_state(model))
            .config_options(Self::all_tool_config_options(&session.enabled_tools)))
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
        let inherited_tools = source.enabled_tools.clone();
        self.session_store.put(&new_session_id, &XaiSessionData {
            cwd,
            model: inherited_model.clone(),
            history: source.history,
            api_key: source.api_key,
            system_prompt: source.system_prompt,
            enabled_tools: inherited_tools.clone(),
            // Clear last_response_id — fork starts without prior xAI response context.
            last_response_id: None,
        }).await.map_err(store_error)?;

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
        // Cancel any in-flight prompt before deleting the session so the prompt
        // task unblocks promptly. The prompt's compensation path will find no
        // session (already deleted) and skip silently — no corruption.
        let cancel_arc = self.cancel_channels.lock().await.remove(&session_id);
        if let Some(arc) = cancel_arc {
            if let Some(tx) = arc.lock().await.take() {
                let _ = tx.send(());
            }
        }
        self.session_store.delete(&session_id).await;
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
        let session_id = req.session_id.to_string();

        // Check if this is one of the known server-side tool toggles.
        let is_known_tool = AVAILABLE_TOOLS.iter().any(|(id, _)| *id == config_id.as_str());

        if is_known_tool {
            let SessionConfigOptionValue::ValueId { value } = req.value else {
                return Err(invalid_params(format!(
                    "{config_id} requires a string value (on/off)"
                )));
            };
            let val = value.to_string();
            if !["off", "on"].contains(&val.as_str()) {
                return Err(invalid_params(format!(
                    "unknown value '{val}' for {config_id}; expected: on, off"
                )));
            }
            let mut session = self.session_store.get(&session_id).await
                .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
            if val == "on" {
                if !session.enabled_tools.iter().any(|t| t == &config_id) {
                    session.enabled_tools.push(config_id.clone());
                }
            } else {
                session.enabled_tools.retain(|t| t != &config_id);
            }
            self.session_store.put(&session_id, &session).await.map_err(store_error)?;
            info!(session_id, tool = %config_id, enabled = (val == "on"), "xai: tool toggled");
            Ok(SetSessionConfigOptionResponse::new(Self::all_tool_config_options(&session.enabled_tools)))
        } else {
            warn!(config_id = %config_id, "xai: set_session_config_option called for unknown option — ignored");
            // Per ACP spec, return the current state of all known options
            // even when the requested config_id is unknown — but the session
            // must still exist.
            let session = self.session_store.get(&session_id).await
                .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
            Ok(SetSessionConfigOptionResponse::new(Self::all_tool_config_options(&session.enabled_tools)))
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
        let enabled_tools = session.enabled_tools.clone();

        // Install a fresh cancel channel for this prompt. Each prompt owns its
        // own Arc so the ptr_eq cleanup at the end can safely distinguish it
        // from a concurrent prompt that may have replaced the HashMap entry.
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        let cancel_arc = Arc::new(Mutex::new(Some(cancel_tx)));
        self.cancel_channels.lock().await.insert(session_id.clone(), cancel_arc.clone());

        // Idempotency check: if history already ends with the exact user message,
        // we are resuming after a crash or a failed put(assistant) from a previous
        // attempt. Skip the write and do not duplicate the message in the xAI
        // context — history already carries it as the last entry.
        let resuming = session.history.last()
            .map(|m| m.role == "user" && m.content_str() == user_input)
            == Some(true);

        if !resuming {
            // Persist the user message BEFORE calling xAI so that a crash between
            // the request and the response cannot silently discard the user's input.
            let mut snapshot = session.clone();
            snapshot.history.push(Message::user(user_input.clone()));
            self.session_store.put(&session_id, &snapshot).await.map_err(store_error)?;
        }

        // Build the Responses API `input` array and choose `previous_response_id`.
        //
        // When a `last_response_id` is stored, the xAI server already holds the
        // full prior context — send only the new user message and reference the
        // prior response. This avoids re-sending the entire history on every turn.
        //
        // When there is no `last_response_id` (new session, forked session, or
        // first turn), send the complete history converted to `InputItem`s.
        //
        // These are `mut` because Gap 3 (stale-ID retry) and Gap 4 (Incomplete
        // continuation) may update them across outer loop iterations.
        let (mut current_input, mut current_prev_response_id) =
            if let Some(prev_id) = &session.last_response_id {
                // Stateful path: one item for this turn + server-held context.
                (vec![InputItem::user(user_input.clone())], Some(prev_id.clone()))
            } else {
                // Full-history path: reconstruct the Responses API input from
                // the stored Message history.
                (build_full_history_input(&session, &user_input, resuming), None)
            };

        let client = Arc::clone(&self.client);
        let mut assistant_text = String::new();
        let mut canceled = false;
        // Set when xAI returns an error event. We break out of the loop instead
        // of returning immediately so the compensation path below can remove the
        // orphaned user message before we propagate the error.
        let mut stream_error: Option<Error> = None;
        // ID returned by xAI for this response; saved as `last_response_id` so
        // subsequent turns can use stateful multi-turn via `previous_response_id`.
        let mut current_response_id: Option<String> = None;
        // Gap 3: set after the first stale-ID retry to prevent infinite retries.
        let mut stale_retry_done = false;
        // Gap 4: counts client-driven continuation requests for Incomplete responses.
        let mut continuations: u32 = 0;
        const MAX_CONTINUATIONS: u32 = 5;

        // Outer loop — normally executes once. Re-runs when:
        //   Gap 3: a stale `previous_response_id` causes an error → retry with
        //          full history and no ID (transparent recovery).
        //   Gap 4: the model response is Incomplete → send a follow-up request
        //          referencing the partial response so the model continues.
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
            // Accumulates (call_id, name) for every tool call opened this turn.
            // Resolved as Completed when the stream ends — server-side tools never
            // emit an explicit completion event visible to the client.
            let mut pending_tool_calls: Vec<(String, String)> = Vec::new();

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
                                canceled = true;
                                break 'outer StopReason::EndTurn;
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
                        if let Err(e) = nats_client.session_notification(notif).await {
                            warn!(session_id, error = %e, "xai: failed to send text notification");
                        }
                    }
                    XaiEvent::ResponseId { id } => {
                        current_response_id = Some(id);
                    }
                    XaiEvent::FunctionCall { call_id, name, arguments } => {
                        info!(session_id, call_id = %call_id, tool_name = %name, "xai: tool call");
                        // Track for resolution at stream end (see XaiEvent::Done).
                        pending_tool_calls.push((call_id.clone(), name.clone()));
                        // Notify the ACP client for observability. Server-side tools
                        // are executed by xAI; the stream continues after completion.
                        let tool_call = ToolCall::new(call_id.clone(), name.clone())
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
                    XaiEvent::Finished { reason } => {
                        info!(session_id, reason = ?reason, "xai: finish reason");
                        // Gap 4: mark for continuation — the outer loop will send
                        // a follow-up request after this stream ends.
                        if reason == FinishReason::Incomplete {
                            needs_continuation = true;
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
                    XaiEvent::Done => {
                        // Close the lifecycle of every tool call opened this turn.
                        // Server-side tools complete internally — no explicit
                        // completion event is emitted, so the client would otherwise
                        // see a Pending tool call that never resolves.
                        for (call_id, name) in pending_tool_calls.drain(..) {
                            let notif = SessionNotification::new(
                                session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new(call_id, name)
                                        .status(ToolCallStatus::Completed)
                                        .kind(ToolKind::Other),
                                ),
                            );
                            if let Err(e) = nats_client.session_notification(notif).await {
                                warn!(session_id, error = %e, "xai: failed to resolve tool call");
                            }
                        }
                        break StopReason::EndTurn;
                    }
                    XaiEvent::Error { message } => {
                        // Gap 3: if we used previous_response_id and get an error on
                        // the first attempt, retry without it using full history.
                        // This transparently recovers from stale or expired response IDs.
                        if current_prev_response_id.is_some() && !stale_retry_done {
                            warn!(session_id, error = %message,
                                "xai: error with previous_response_id — retrying with full history");
                            stale_retry_done = true;
                            current_prev_response_id = None;
                            current_input = build_full_history_input(&session, &user_input, resuming);
                            continue 'outer;
                        }
                        tracing::error!(session_id, error = %message, "xai: stream error");
                        stream_error = Some(internal_error(message));
                        break StopReason::EndTurn;
                    }
                }
            };

            // Gap 4: if the model was cut short, send a follow-up request so it
            // continues from where it stopped. The server holds the full context
            // via previous_response_id — no history re-send needed.
            if needs_continuation && continuations < MAX_CONTINUATIONS {
                if let Some(resp_id) = current_response_id.clone() {
                    info!(session_id, continuations, "xai: response incomplete — sending continuation request");
                    current_prev_response_id = Some(resp_id);
                    // Empty input — context is carried by previous_response_id.
                    current_input = vec![];
                    continuations += 1;
                    continue 'outer;
                }
            }

            break 'outer inner_stop;
        };

        // Compensation: remove the orphaned user message when the turn did not
        // produce a persisted assistant reply. Covers both cancel and xAI errors.
        // This is best-effort — a crash here leaves an orphaned user message,
        // which is acceptable (the message was sent; the turn was just cut short).
        let needs_compensation = canceled || stream_error.is_some();
        if needs_compensation {
            if let Some(mut current) = self.session_store.get(&session_id).await {
                if current.history.last().map(|m| m.role == "user" && m.content_str() == user_input)
                    == Some(true)
                {
                    current.history.pop();
                    let _ = self.session_store.put(&session_id, &current).await;
                }
            }
        } else if !assistant_text.is_empty() {
            // Re-read to preserve any concurrent model/config changes, then append
            // the assistant reply and update `last_response_id` for the next turn.
            //
            // Note: a crash between here and the put below would leave the user
            // message in history without a corresponding assistant reply. This
            // window is inherent to the streaming-then-persist design — the
            // assistant text lives in memory until this write completes.
            if let Some(mut current) = self.session_store.get(&session_id).await {
                current.history.push(Message::assistant_text(assistant_text));
                trim_history(&mut current.history, self.max_history_messages);
                // Update last_response_id so the next turn can use stateful multi-turn
                // without re-sending the full history.
                if current_response_id.is_some() {
                    current.last_response_id = current_response_id;
                }
                self.session_store.put(&session_id, &current).await.map_err(store_error)?;
            }
        }

        // Release the cancel channel entry now that the prompt is done.
        // Using ptr_eq so that a concurrent prompt (outside the ACP contract,
        // but possible) that already replaced our Arc is not accidentally removed.
        {
            let mut channels = self.cancel_channels.lock().await;
            if channels.get(&session_id)
                .map(|a| Arc::ptr_eq(a, &cancel_arc))
                .unwrap_or(false)
            {
                channels.remove(&session_id);
            }
        }

        if let Some(e) = stream_error {
            return Err(e);
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

/// Build the full `input` array for a Responses API request from stored session history.
///
/// Used when there is no `previous_response_id` (new session, forked session, or
/// fallback after a stale-ID error). Prepends the system prompt if present, then
/// converts each `Message` in history to the corresponding `InputItem` variant.
fn build_full_history_input(
    session: &crate::session_store::XaiSessionData,
    user_input: &str,
    resuming: bool,
) -> Vec<InputItem> {
    let mut items: Vec<InputItem> = Vec::new();
    if let Some(sp) = &session.system_prompt {
        items.push(InputItem::system(sp.clone()));
    }
    for msg in &session.history {
        match msg.role.as_str() {
            "user" => items.push(InputItem::user(msg.content_str())),
            "assistant" => {
                if let Some(tcs) = &msg.tool_calls {
                    for tc in tcs {
                        items.push(InputItem::function_call(
                            &tc.id,
                            &tc.function.name,
                            &tc.function.arguments,
                        ));
                    }
                } else {
                    items.push(InputItem::assistant(msg.content_str()));
                }
            }
            "tool" => {
                if let Some(tid) = &msg.tool_call_id {
                    items.push(InputItem::function_call_output(tid, msg.content_str()));
                }
            }
            _ => {}
        }
    }
    // When resuming, the user message is already the last item in history.
    if !resuming {
        items.push(InputItem::user(user_input.to_string()));
    }
    items
}

/// Trim `history` in-place so it contains at most `max` messages.
///
/// Messages are removed from the front in structure-aware increments:
/// - A leading `user` + `assistant` pair is removed together (2 at a time).
/// - A leading orphaned `user` message (no corresponding assistant — left over
///   from a timed-out or crashed turn) is removed individually (1 at a time).
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

    // ── trim_history ──────────────────────────────────────────────────────────

    fn msg(role: &str, content: &str) -> Message {
        Message { role: role.to_string(), content: Some(content.to_string()), tool_calls: None, tool_call_id: None }
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
        // [u1,a1, u2,a2, u3,a3], max=4 → remove first pair → [u2,a2, u3,a3]
        let mut h = vec![
            msg("user","u1"), msg("assistant","a1"),
            msg("user","u2"), msg("assistant","a2"),
            msg("user","u3"), msg("assistant","a3"),
        ];
        trim_history(&mut h, 4);
        assert_eq!(h.len(), 4);
        assert_eq!(h[0].content_str(),"u2");
    }

    #[test]
    fn trim_history_odd_max_removes_full_pair() {
        // max=3, 4 messages → excess=1 → remove one pair → 2 remain (not 3)
        let mut h = vec![
            msg("user","u1"), msg("assistant","a1"),
            msg("user","u2"), msg("assistant","a2"),
        ];
        trim_history(&mut h, 3);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].content_str(),"u2");
        assert_eq!(h[1].content_str(),"a2");
    }

    #[test]
    fn trim_history_orphaned_user_removed_individually() {
        // [user_orphan, user_new, assistant_new], max=2
        // → remove orphan → [user_new, assistant_new]
        let mut h = vec![
            msg("user","orphan"),
            msg("user","new"),
            msg("assistant","reply"),
        ];
        trim_history(&mut h, 2);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].content_str(),"new");
        assert_eq!(h[1].content_str(),"reply");
    }

    #[test]
    fn trim_history_multiple_orphans_then_pair() {
        // [orphan1, orphan2, user, assistant], max=2
        // → remove orphan1 → remove orphan2 → [user, assistant]
        let mut h = vec![
            msg("user","orphan1"),
            msg("user","orphan2"),
            msg("user","new"),
            msg("assistant","reply"),
        ];
        trim_history(&mut h, 2);
        assert_eq!(roles(&h), vec!["user", "assistant"]);
        assert_eq!(h[0].content_str(),"new");
    }

    #[test]
    fn trim_history_stops_on_unexpected_leading_role() {
        // assistant at front (malformed) — do not touch
        let mut h = vec![msg("assistant","a"), msg("user","u")];
        trim_history(&mut h, 1);
        assert_eq!(h.len(), 2, "should not trim malformed leading assistant");
    }

    #[test]
    fn trim_history_empty_is_noop() {
        let mut h: Vec<Message> = vec![];
        trim_history(&mut h, 0);
        assert!(h.is_empty());
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

    pub async fn test_set_session_history(&self, id: &str, history: Vec<Message>) {
        if let Some(mut data) = self.session_store.get(id).await {
            data.history = history;
            self.session_store.put(id, &data).await.expect("test_set_session_history: put failed");
        }
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

    pub async fn test_cancel_channels_len(&self) -> usize {
        self.cancel_channels.lock().await.len()
    }
}
