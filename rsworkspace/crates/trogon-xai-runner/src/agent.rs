use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
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
    SessionListCapabilities, SessionMode, SessionModeState, SessionModelState, SessionNotification,
    SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
    SetSessionModelRequest, SetSessionModelResponse, StopReason, ToolCall, ToolCallStatus,
    ToolKind, UsageUpdate,
};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::agent_loader::{AgentLoader, AgentLoading};
use crate::client::{FinishReason, InputItem, Message, XaiClient, XaiEvent};
use crate::console_session::{
    ConsoleSessionWriting, KvConsoleSessionWriter, RawMessage, RawSession,
};
use crate::http_client::XaiHttpClient;
use crate::session_notifier::{NatsSessionNotifier, SessionNotifier};
use crate::session_store::{KvSessionStore, SessionStore, XaiSessionData};
use crate::skill_loader::{SkillLoader, SkillLoading};

type CancelChannels = Arc<Mutex<HashMap<String, Arc<Mutex<Option<oneshot::Sender<()>>>>>>>;
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
const AVAILABLE_TOOLS: &[(&str, &str)] = &[("web_search", "Web Search"), ("x_search", "X Search")];

/// ACP Agent implementation backed by xAI's Grok API (Responses API).
///
/// Each `XaiAgent` manages multiple sessions, each holding its own conversation
/// history persisted in NATS KV. Prompt calls stream chat completions from
/// `api.x.ai` and forward text chunks to the ACP client via `SessionNotification`s.
///
/// - `H` abstracts the xAI HTTP client (default: `XaiClient`).
/// - `N` abstracts the ACP session notification channel (default: `NatsSessionNotifier`).
///
/// Production code uses `XaiAgent<XaiClient, NatsSessionNotifier>` (the defaults).
pub struct XaiAgent<H = XaiClient, N = NatsSessionNotifier> {
    notifier: Arc<N>,
    client: Arc<H>,
    session_store: Box<dyn SessionStore>,
    /// In-memory cancel channels — one per active prompt, keyed by session id.
    cancel_channels: CancelChannels,
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
    /// **Safe only when a single client is active at a time, or when all clients
    /// use the `"agent"` auth method (server key).** If multiple clients
    /// concurrently call `authenticate("xai-api-key")` before either calls
    /// `new_session`, the second write overwrites the first — the first client
    /// silently inherits the second client's key.
    ///
    /// The root cause is that the ACP protocol carries no per-connection
    /// identifier: `authenticate` and `new_session` arrive as independent NATS
    /// messages on shared subjects, so there is no way to correlate them at the
    /// application layer without a protocol change (e.g. `authenticate` returning
    /// an opaque correlation token that `new_session` echoes back).
    pending_api_key: Arc<Mutex<Option<String>>>,
    /// Optional system prompt injected at the start of every conversation.
    /// Read from `XAI_SYSTEM_PROMPT` at construction time.
    system_prompt: Option<String>,
    /// Maximum number of messages kept in history (user + assistant interleaved).
    /// Oldest messages are trimmed in pairs to preserve user/assistant ordering.
    /// Configured via `XAI_MAX_HISTORY_MESSAGES` (default: 20 = 10 exchanges).
    ///
    /// Note: truncation is by message count, not tokens. With server-side tools
    /// (`enabled_tools`) active, messages can be long; lower this value if
    /// context window errors occur.
    max_history_messages: usize,
    /// Maximum agentic tool-call turns per prompt.
    ///
    /// Passed as `max_turns` in the Responses API request when tools are enabled.
    /// The xAI server caps its internal tool-calling loop at this value.
    /// Configured via `XAI_MAX_TURNS` (default: 10; 0 = use server default).
    max_turns: Option<u32>,
    /// Optional console agent loader. When `CONSOLE_AGENT_ID` is set at startup,
    /// reads skill IDs from the `CONSOLE_AGENTS` KV bucket on each new session.
    agent_loader: Option<Arc<AgentLoader>>,
    /// Optional console skill loader. Resolves skill IDs to formatted content
    /// from `CONSOLE_SKILLS` / `CONSOLE_SKILL_VERSIONS` KV buckets.
    skill_loader: Option<Arc<SkillLoader>>,
    /// Agent ID to look up in `CONSOLE_AGENTS`. Read from `CONSOLE_AGENT_ID` env var.
    console_agent_id: Option<String>,
    /// Writes `RawSession` entries to the console `SESSIONS` KV bucket so
    /// trogon-console can display live session state.
    console_session_writer: Option<Arc<dyn ConsoleSessionWriting>>,
}

impl XaiAgent<XaiClient, NatsSessionNotifier> {
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
        let bucket =
            std::env::var("XAI_SESSION_BUCKET").unwrap_or_else(|_| "XAI_SESSIONS".to_string());
        let session_ttl = std::env::var("XAI_SESSION_TTL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(7 * 24 * 3600));
        let js = async_nats::jetstream::new(nats.clone());
        let store = KvSessionStore::open(js.clone(), bucket, session_ttl).await?;
        let mut agent = Self::new_with_store(nats, acp_prefix, default_model, api_key, Box::new(store));

        if let Ok(agent_id) = std::env::var("CONSOLE_AGENT_ID") {
            if !agent_id.is_empty() {
                match (
                    AgentLoader::open(&js).await,
                    SkillLoader::open(&js).await,
                    KvConsoleSessionWriter::open(&js).await,
                ) {
                    (Ok(al), Ok(sl), Ok(sw)) => {
                        info!(agent_id = %agent_id, "xai: console integration enabled");
                        agent.agent_loader = Some(Arc::new(al));
                        agent.skill_loader = Some(Arc::new(sl));
                        agent.console_session_writer = Some(Arc::new(sw));
                        agent.console_agent_id = Some(agent_id);
                    }
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                        warn!(error = %e, "xai: failed to open console KV buckets — console integration disabled");
                    }
                }
            }
        }

        Ok(agent)
    }

    /// Internal constructor: KV-backed store + default `XaiClient` + `NatsSessionNotifier`.
    fn new_with_store(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        session_store: Box<dyn SessionStore>,
    ) -> Self {
        let notifier = NatsSessionNotifier::new(nats, acp_prefix);
        XaiAgent::new_with_client_and_store(
            notifier,
            default_model,
            api_key,
            session_store,
            XaiClient::new(),
        )
    }
}

impl<H: XaiHttpClient, N: SessionNotifier> XaiAgent<H, N> {
    /// Generic constructor: takes an explicit HTTP client `H`, session notifier `N`,
    /// and session store.
    ///
    /// This is the single source of truth for initialising all fields. Every
    /// other constructor ultimately delegates here.
    pub(crate) fn new_with_client_and_store(
        notifier: N,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        session_store: Box<dyn SessionStore>,
        client: H,
    ) -> Self {
        let default_model: String = default_model.into();
        let api_key_str: String = api_key.into();
        let global_api_key = if api_key_str.is_empty() {
            None
        } else {
            Some(api_key_str)
        };

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

        let max_history_messages = std::env::var("XAI_MAX_HISTORY_MESSAGES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(20);

        let max_turns = match std::env::var("XAI_MAX_TURNS") {
            Ok(s) => match s.parse::<u32>() {
                Ok(0) => None,      // 0 = server default: omit max_turns field
                Ok(n) => Some(n),   // explicit positive value
                Err(_) => Some(10), // invalid → app default
            },
            Err(_) => Some(10), // unset → app default
        };

        Self {
            notifier: Arc::new(notifier),
            client: Arc::new(client),
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
            agent_loader: None,
            skill_loader: None,
            console_agent_id: None,
            console_session_writer: None,
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

    /// Write `data` to the console SESSIONS KV bucket as a `RawSession`.
    /// No-op when console integration is disabled. Best-effort: errors are logged.
    async fn write_console_session(&self, session_id: &str, data: &XaiSessionData) {
        let Some(writer) = &self.console_session_writer else { return };
        let Some(tenant_id) = self.console_agent_id.as_deref() else { return };

        let now = crate::console_session::now_secs();
        let messages = data
            .history
            .iter()
            .map(|m| RawMessage {
                role: m.role.clone(),
                content: m.content.clone().unwrap_or_default(),
            })
            .collect();

        let raw = RawSession {
            id: session_id.to_string(),
            tenant_id: tenant_id.to_string(),
            name: format!("Session {}", &session_id[..8.min(session_id.len())]),
            model: data.model.clone(),
            agent_id: self.console_agent_id.clone(),
            messages,
            created_at: crate::console_session::secs_to_iso8601(data.created_at_secs),
            updated_at: crate::console_session::secs_to_iso8601(now),
            duration_ms: now.saturating_sub(data.created_at_secs) * 1000,
        };

        let key = format!("{tenant_id}.{session_id}");
        writer.write(&key, &raw).await;
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
                    return Err(auth_required("authenticate: XAI_API_KEY must not be empty"));
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
                return Err(invalid_params(format!(
                    "authenticate: unknown method '{other}'"
                )));
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

        let api_key = self
            .pending_api_key
            .lock()
            .await
            .take()
            .or_else(|| self.global_api_key.clone());

        let (session_model, skill_text, agent_system_prompt) =
            match (&self.agent_loader, &self.skill_loader, &self.console_agent_id) {
                (Some(al), Some(sl), Some(id)) => {
                    let cfg = al.get_agent_config(id).await;
                    let skill_text = sl.load(&cfg.skill_ids).await;
                    (cfg.model_id, skill_text, cfg.system_prompt)
                }
                _ => (None, None, None),
            };

        // Build the combined system prompt: skills → agent prompt → global env prompt.
        // Each layer is separated by "---" only when both sides are non-empty.
        let session_system_prompt = [skill_text, agent_system_prompt, self.system_prompt.clone()]
            .into_iter()
            .flatten()
            .reduce(|acc, next| format!("{acc}\n\n---\n\n{next}"));

        let created_at_secs = crate::console_session::now_secs();
        let data = XaiSessionData {
            cwd,
            model: session_model,
            history: Vec::new(),
            api_key,
            system_prompt: session_system_prompt,
            enabled_tools: Vec::new(),
            last_response_id: None,
            created_at_secs,
        };
        self.session_store
            .put(&session_id, &data)
            .await
            .map_err(store_error)?;
        self.write_console_session(&session_id, &data).await;

        info!(session_id, "xai: new session");
        Ok(NewSessionResponse::new(SessionId::from(session_id))
            .modes(self.session_mode_state())
            .models(self.session_model_state(data.model.as_deref()))
            .config_options(Self::all_tool_config_options(&[])))
    }

    async fn load_session(
        &self,
        req: LoadSessionRequest,
    ) -> agent_client_protocol::Result<LoadSessionResponse> {
        let session_id = req.session_id.to_string();
        let session = self
            .session_store
            .get(&session_id)
            .await
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
        self.session_store
            .get(&session_id)
            .await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
        Ok(ResumeSessionResponse::new())
    }

    async fn fork_session(
        &self,
        req: ForkSessionRequest,
    ) -> agent_client_protocol::Result<ForkSessionResponse> {
        let source_id = req.session_id.to_string();
        let cwd = req.cwd.to_string_lossy().into_owned();

        let source = self
            .session_store
            .get(&source_id)
            .await
            .ok_or_else(|| not_found(format!("session {source_id} not found")))?;

        let new_session_id = Uuid::new_v4().to_string();
        let inherited_model = source.model.clone();
        let inherited_tools = source.enabled_tools.clone();
        let forked_data = XaiSessionData {
            cwd,
            model: inherited_model.clone(),
            history: source.history,
            api_key: source.api_key,
            system_prompt: source.system_prompt,
            enabled_tools: inherited_tools.clone(),
            last_response_id: None,
            created_at_secs: crate::console_session::now_secs(),
        };
        self.session_store
            .put(&new_session_id, &forked_data)
            .await
            .map_err(store_error)?;
        self.write_console_session(&new_session_id, &forked_data).await;

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
        if let Some(arc) = cancel_arc
            && let Some(tx) = arc.lock().await.take()
        {
            let _ = tx.send(());
        }
        self.session_store.delete(&session_id).await;
        info!(session_id, "xai: session closed");
        Ok(CloseSessionResponse::new())
    }

    async fn list_sessions(
        &self,
        _req: ListSessionsRequest,
    ) -> agent_client_protocol::Result<ListSessionsResponse> {
        let list = self
            .session_store
            .list()
            .await
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
        self.session_store
            .get(&session_id)
            .await
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

        if !self
            .available_models
            .iter()
            .any(|m| m.model_id.0.as_ref() == model_id)
        {
            return Err(invalid_params(format!("unknown model: {model_id}")));
        }

        let mut session = self
            .session_store
            .get(&session_id)
            .await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
        session.model = Some(model_id.clone());
        // response IDs are model-specific — a stale ID from the previous model
        // would always trigger a retry on the next prompt. Clear it proactively.
        session.last_response_id = None;
        self.session_store
            .put(&session_id, &session)
            .await
            .map_err(store_error)?;
        self.write_console_session(&session_id, &session).await;

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
        let is_known_tool = AVAILABLE_TOOLS
            .iter()
            .any(|(id, _)| *id == config_id.as_str());

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
            let mut session = self
                .session_store
                .get(&session_id)
                .await
                .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
            if val == "on" {
                if !session.enabled_tools.iter().any(|t| t == &config_id) {
                    session.enabled_tools.push(config_id.clone());
                }
            } else {
                session.enabled_tools.retain(|t| t != &config_id);
            }
            self.session_store
                .put(&session_id, &session)
                .await
                .map_err(store_error)?;
            info!(session_id, tool = %config_id, enabled = (val == "on"), "xai: tool toggled");
            Ok(SetSessionConfigOptionResponse::new(
                Self::all_tool_config_options(&session.enabled_tools),
            ))
        } else {
            warn!(config_id = %config_id, "xai: set_session_config_option called for unknown option — ignored");
            // Per ACP spec, return the current state of all known options
            // even when the requested config_id is unknown — but the session
            // must still exist.
            let session = self
                .session_store
                .get(&session_id)
                .await
                .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
            Ok(SetSessionConfigOptionResponse::new(
                Self::all_tool_config_options(&session.enabled_tools),
            ))
        }
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
            warn!(
                session_id,
                "xai: prompt contains no text or resource blocks"
            );
        }

        // Read session state.
        let session = self
            .session_store
            .get(&session_id)
            .await
            .ok_or_else(|| not_found(format!("session {session_id} not found")))?;

        let api_key = session.api_key.clone().ok_or_else(|| {
            auth_required(
                "no API key for this session: authenticate with method 'xai-api-key' first",
            )
        })?;

        let model = session
            .model
            .as_deref()
            .unwrap_or(&self.default_model)
            .to_string();
        let enabled_tools = session.enabled_tools.clone();

        // Install a fresh cancel channel for this prompt. Each prompt owns its
        // own Arc so the ptr_eq cleanup at the end can safely distinguish it
        // from a concurrent prompt that may have replaced the HashMap entry.
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        let cancel_arc = Arc::new(Mutex::new(Some(cancel_tx)));
        self.cancel_channels
            .lock()
            .await
            .insert(session_id.clone(), cancel_arc.clone());

        // Idempotency check: if history already ends with the exact user message,
        // we are resuming after a crash or a failed put(assistant) from a previous
        // attempt. Skip the write and do not duplicate the message in the xAI
        // context — history already carries it as the last entry.
        let resuming = session
            .history
            .last()
            .map(|m| m.role == "user" && m.content_str() == user_input)
            == Some(true);

        if !resuming {
            // Persist the user message BEFORE calling xAI so that a crash between
            // the request and the response cannot silently discard the user's input.
            let mut snapshot = session.clone();
            snapshot.history.push(Message::user(user_input.clone()));
            self.session_store
                .put(&session_id, &snapshot)
                .await
                .map_err(store_error)?;
            // Publish user message so console shows Running status.
            self.write_console_session(&session_id, &snapshot).await;
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
        // These are `mut` because the stale-ID retry and Incomplete continuation
        // may update them across outer loop iterations.
        let (mut current_input, mut current_prev_response_id) =
            if let Some(prev_id) = &session.last_response_id {
                // Stateful path: one item for this turn + server-held context.
                (
                    vec![InputItem::user(user_input.clone())],
                    Some(prev_id.clone()),
                )
            } else {
                // Full-history path: reconstruct the Responses API input from
                // the stored Message history.
                (
                    build_full_history_input(&session, &user_input, resuming),
                    None,
                )
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
        // Set after the first stale-ID retry to prevent infinite retries.
        let mut stale_retry_done = false;
        // Set when the outer loop is executing a continuation request (Incomplete
        // response). Disables the stale-ID retry for that iteration: an error
        // during continuation should surface to the user, not silently restart
        // the model from scratch with full history.
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
            // The `incomplete_details.reason` from the last Finished(Incomplete)
            // event. Drives the continuation input strategy:
            // - "max_output_tokens" → empty input (model resumes its own text)
            // - "max_turns" / other → re-send user question (model has tool
            //   results in context but hasn't produced the final answer yet)
            let mut last_incomplete_reason: Option<String> = None;
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
                                // Do NOT set canceled=true. Unlike an explicit cancel,
                                // a timeout may have produced partial text that should be
                                // preserved (the else-if branch below saves it).
                                // StopReason::Cancelled signals to the ACP client that
                                // the prompt did not complete normally.
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
                        self.notifier.session_notification(notif).await;
                    }
                    XaiEvent::ResponseId { id } => {
                        current_response_id = Some(id);
                    }
                    XaiEvent::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    } => {
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
                        self.notifier.session_notification(notif).await;
                    }
                    XaiEvent::ServerToolCompleted { name } => {
                        // A server-side built-in tool finished on xAI's infrastructure.
                        // Advance the matching pending tool call to Completed now —
                        // before the model starts streaming its text answer — so the
                        // ACP client gets real-time status instead of waiting up to
                        // 30 s for [DONE].
                        //
                        // Matching is by tool name (FIFO) because the search call
                        // events carry an `item_id` that differs from the `call_id`
                        // in the `function_call` event. xAI executes server-side
                        // tool calls sequentially so FIFO order is always correct.
                        // If no matching entry exists (e.g. the server sent a
                        // ServerToolCompleted without a prior FunctionCall for some
                        // reason), this is a silent no-op — not an error.
                        if let Some(pos) = pending_tool_calls.iter().position(|(_, n)| *n == name) {
                            let (call_id, tc_name) = pending_tool_calls.remove(pos);
                            info!(session_id, call_id = %call_id, tool_name = %tc_name,
                                  "xai: search call completed on server — advancing tool to Completed");
                            let notif = SessionNotification::new(
                                session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new(call_id, tc_name)
                                        .status(ToolCallStatus::Completed)
                                        .kind(ToolKind::Other),
                                ),
                            );
                            self.notifier.session_notification(notif).await;
                        }
                    }
                    XaiEvent::Finished {
                        reason,
                        incomplete_reason,
                    } => {
                        info!(session_id, reason = ?reason, incomplete_reason = ?incomplete_reason, "xai: finish reason");
                        match reason {
                            FinishReason::Incomplete => {
                                needs_continuation = true;
                                last_incomplete_reason = incomplete_reason;
                            }
                            FinishReason::Failed => {
                                // The model itself failed (not a network error).
                                // Surface as a stream error so the compensation
                                // path removes the orphaned user message and the
                                // ACP client receives an explicit error instead
                                // of a silent empty response.
                                stream_error = Some(internal_error("xAI response failed"));
                            }
                            FinishReason::Cancelled => {
                                // xAI cancelled the response server-side (safety filter,
                                // content policy, quota). Not the same as our client-side
                                // cancel — the user's prompt was rejected. Surface as an
                                // error so the caller knows why they got no output.
                                stream_error = Some(internal_error("xAI cancelled the response"));
                            }
                            FinishReason::Completed => {}
                            FinishReason::Other(ref s) => {
                                warn!(session_id, status = %s, "xai: unknown finish status — treating as end of turn");
                            }
                        }
                    }
                    XaiEvent::Usage {
                        prompt_tokens,
                        completion_tokens,
                    } => {
                        info!(
                            session_id,
                            prompt_tokens, completion_tokens, "xai: token usage"
                        );
                        // UsageUpdate(used, size): used = tokens currently in context
                        // (prompt_tokens from xAI = full input sent this turn);
                        // size = 0 because the model's context window limit is not
                        // returned by the Responses API.
                        let notif = SessionNotification::new(
                            session_id.clone(),
                            SessionUpdate::UsageUpdate(UsageUpdate::new(prompt_tokens, 0)),
                        );
                        self.notifier.session_notification(notif).await;
                    }
                    XaiEvent::Done => {
                        // Close the lifecycle of every tool call opened this turn.
                        // Server-side tools complete internally — no explicit
                        // completion event is emitted, so the client would otherwise
                        // see a Pending tool call that never resolves.
                        // Use Failed when a stream error was already recorded (e.g.
                        // Finished { Failed } arrived before [DONE]) — marking tools
                        // Completed in that case would be misleading to the ACP client.
                        let tool_status = if stream_error.is_some() {
                            ToolCallStatus::Failed
                        } else {
                            ToolCallStatus::Completed
                        };
                        for (call_id, name) in pending_tool_calls.drain(..) {
                            let notif = SessionNotification::new(
                                session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new(call_id, name)
                                        .status(tool_status)
                                        .kind(ToolKind::Other),
                                ),
                            );
                            self.notifier.session_notification(notif).await;
                        }
                        break StopReason::EndTurn;
                    }
                    XaiEvent::Error { message } => {
                        // If we used a session-carried previous_response_id (not a
                        // continuation ID) and get an error on the first attempt,
                        // retry transparently with full history — the stored ID may
                        // have expired. Only fires once and never during continuations:
                        // `continuation_in_progress` ensures a continuation error
                        // surfaces to the caller instead of silently restarting the
                        // model from scratch with a different response.
                        //
                        // 4xx errors (auth failure, bad request, quota) are NOT retried:
                        // the stale-ID theory only applies to context/expiry errors. A
                        // 401/403 would just repeat with the same key and fail again,
                        // wasting one API call. The heuristic is anchored to the exact
                        // prefix produced by start_request ("xAI API error 4XX:…") to
                        // avoid false-positive matches on body text that contains "error 4".
                        let is_client_error = message.contains("xAI API error 4");
                        if current_prev_response_id.is_some()
                            && !stale_retry_done
                            && !continuation_in_progress
                            && !is_client_error
                        {
                            warn!(session_id, error = %message,
                                "xai: error with previous_response_id — retrying with full history");
                            stale_retry_done = true;
                            // Clear any text accumulated before the error (e.g. from a
                            // partial Incomplete turn) — the retry produces a fresh response
                            // and must not be concatenated with the truncated text.
                            assistant_text.clear();
                            // Also clear current_response_id: if the failed stream emitted
                            // an ID before the error, that ID is from a failed request and
                            // must not be saved as last_response_id. If the retry succeeds
                            // it will set a fresh ID; if it fails too, None is the right
                            // value (forces full-history rebuild on the next prompt).
                            current_response_id = None;
                            current_prev_response_id = None;
                            current_input =
                                build_full_history_input(&session, &user_input, resuming);
                            continue 'outer;
                        }
                        tracing::error!(session_id, error = %message, "xai: stream error");
                        stream_error = Some(internal_error(message));
                        break StopReason::EndTurn;
                    }
                }
            };

            // Resolve any tool calls that were left open because the stream ended
            // without a [DONE] event (network cut, server truncation) or because
            // a stream error fired before [DONE] arrived. In the normal path,
            // XaiEvent::Done already drained this vec — this is a no-op there.
            // cancel/timeout break via 'outer and never reach here; those are
            // abrupt interruptions where leaving tool calls unresolved is acceptable.
            // Use Failed when a stream error is set — tool calls did not complete.
            let fallback_tool_status = if stream_error.is_some() {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            };
            for (call_id, name) in pending_tool_calls.drain(..) {
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCall(
                        ToolCall::new(call_id, name)
                            .status(fallback_tool_status)
                            .kind(ToolKind::Other),
                    ),
                );
                self.notifier.session_notification(notif).await;
            }

            // If the model was cut short, send a follow-up request so it
            // continues. The server holds the full context via previous_response_id.
            //
            // Continuation input depends on WHY the response was incomplete:
            // - "max_output_tokens": the model was truncated mid-text. Send an
            //   empty input so the model resumes exactly where it stopped.
            //   Re-sending the user question would make the model restart.
            // - "max_turns" (or unknown): tool-call iterations were exhausted.
            //   The model has all tool results in context but hasn't answered yet.
            //   Re-send the original question so the model produces the final answer.
            if needs_continuation && stream_error.is_none() {
                // Guard: prevent infinite loops when the model is repeatedly
                // Incomplete. When MAX_CONTINUATIONS is exhausted the partial
                // text accumulated so far is saved (the else-if branch below)
                // and the caller receives Cancelled — distinct from EndTurn so
                // the ACP client knows the output is incomplete.
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
                        "xai: response incomplete but no response_id received — cannot continue, returning truncated response"
                    );
                }
            }

            break 'outer inner_stop;
        };

        // Compensation: remove the orphaned user message when the turn did not
        // produce a persisted assistant reply. Covers both cancel and xAI errors.
        // This is best-effort — a crash here leaves an orphaned user message,
        // which is acceptable (the message was sent; the turn was just cut short).
        //
        // Only compensate when this call actually wrote the user message (!resuming).
        // When resuming=true the message was already in history from a prior call
        // that crashed — this call did not write it, so popping it would silently
        // discard a message that belongs to the previous turn.
        let needs_compensation = !resuming && (canceled || stream_error.is_some());
        if needs_compensation && let Some(mut current) = self.session_store.get(&session_id).await {
            if current
                .history
                .last()
                .map(|m| m.role == "user" && m.content_str() == user_input)
                == Some(true)
            {
                current.history.pop();
                let _ = self.session_store.put(&session_id, &current).await;
                // Sync console so the phantom user message is removed (status: Idle).
                self.write_console_session(&session_id, &current).await;
            }
        } else if !assistant_text.is_empty() || current_response_id.is_some() {
            // Re-read to preserve any concurrent model/config changes, then:
            // - append the assistant reply to history (only when text was produced)
            // - always update last_response_id when we received one
            //
            // These two concerns are intentionally separated: a turn that only
            // executes server-side tools may produce no text but still yields a
            // valid response ID. Discarding the ID would force the next turn to
            // re-send the full history, losing the tool-execution context.
            //
            // Note: a crash between here and the put below would leave the user
            // message in history without a corresponding assistant reply. This
            // window is inherent to the streaming-then-persist design — the
            // assistant text lives in memory until this write completes.
            if let Some(mut current) = self.session_store.get(&session_id).await {
                if !assistant_text.is_empty() {
                    current
                        .history
                        .push(Message::assistant_text(assistant_text));
                    trim_history(&mut current.history, self.max_history_messages);
                }
                if let Some(resp_id) = current_response_id {
                    current.last_response_id = Some(resp_id);
                }
                self.session_store
                    .put(&session_id, &current)
                    .await
                    .map_err(store_error)?;
                // Publish completed turn so console shows Idle status.
                self.write_console_session(&session_id, &current).await;
            }
        }

        // Release the cancel channel entry now that the prompt is done.
        // Using ptr_eq so that a concurrent prompt (outside the ACP contract,
        // but possible) that already replaced our Arc is not accidentally removed.
        {
            let mut channels = self.cancel_channels.lock().await;
            if channels
                .get(&session_id)
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

    async fn cancel(&self, req: CancelNotification) -> agent_client_protocol::Result<()> {
        let session_id = req.session_id.to_string();
        if let Some(cancel_arc) = self.cancel_channels.lock().await.get(&session_id).cloned()
            && let Some(tx) = cancel_arc.lock().await.take()
        {
            let _ = tx.send(());
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
            "assistant" => items.push(InputItem::assistant(msg.content_str())),
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

#[cfg(feature = "test-helpers")]
impl XaiAgent<XaiClient, NatsSessionNotifier> {
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
        Ok(Self::new_with_store(
            nats,
            acp_prefix,
            default_model,
            api_key,
            Box::new(store),
        ))
    }

    /// Like `new_with_kv_bucket` but points the xAI client at a custom base URL
    /// instead of reading `XAI_BASE_URL` from the environment. Allows KV
    /// integration tests to use a fake HTTP server without touching env vars.
    pub async fn new_with_kv_bucket_and_xai_url(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        bucket: impl Into<String>,
        xai_base_url: impl Into<String>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let js = async_nats::jetstream::new(nats.clone());
        let store = KvSessionStore::open(js, bucket, Duration::from_secs(7 * 24 * 3600)).await?;
        let mut agent =
            Self::new_with_store(nats, acp_prefix, default_model, api_key, Box::new(store));
        agent.client = Arc::new(crate::client::XaiClient::with_base_url(xai_base_url));
        Ok(agent)
    }

    /// Like `new_with_kv_bucket` but with a configurable TTL. For tests only.
    pub async fn new_with_kv_bucket_ttl(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        bucket: impl Into<String>,
        ttl: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let js = async_nats::jetstream::new(nats.clone());
        let store = KvSessionStore::open(js, bucket, ttl).await?;
        Ok(Self::new_with_store(
            nats,
            acp_prefix,
            default_model,
            api_key,
            Box::new(store),
        ))
    }

    /// Creates an `XaiAgent` with a caller-supplied session store. For tests only.
    pub fn new_with_custom_store(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        store: Box<dyn SessionStore>,
    ) -> Self {
        Self::new_with_store(nats, acp_prefix, default_model, api_key, store)
    }
}

#[cfg(feature = "test-helpers")]
impl<H: XaiHttpClient, N: SessionNotifier> XaiAgent<H, N> {
    /// Creates an `XaiAgent` backed by an in-memory session store, an explicit
    /// HTTP client, and an explicit session notifier. For tests only.
    pub fn new_in_memory(
        notifier: N,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        client: H,
    ) -> Self {
        Self::new_with_client_and_store(
            notifier,
            default_model,
            api_key,
            Box::new(MemorySessionStore::new()),
            client,
        )
    }

    pub async fn test_session_history(&self, id: &str) -> Vec<Message> {
        self.session_store
            .get(id)
            .await
            .map(|s| s.history)
            .unwrap_or_default()
    }

    pub async fn test_set_session_history(&self, id: &str, history: Vec<Message>) {
        if let Some(mut data) = self.session_store.get(id).await {
            data.history = history;
            self.session_store
                .put(id, &data)
                .await
                .expect("test_set_session_history: put failed");
        }
    }

    pub async fn test_session_model(&self, id: &str) -> Option<String> {
        self.session_store.get(id).await.and_then(|s| s.model)
    }

    pub async fn test_session_enabled_tools(&self, id: &str) -> Vec<String> {
        self.session_store
            .get(id)
            .await
            .map(|s| s.enabled_tools)
            .unwrap_or_default()
    }

    pub async fn test_session_last_response_id(&self, id: &str) -> Option<String> {
        self.session_store
            .get(id)
            .await
            .and_then(|s| s.last_response_id)
    }

    pub async fn test_set_last_response_id(&self, id: &str, response_id: Option<String>) {
        if let Some(mut data) = self.session_store.get(id).await {
            data.last_response_id = response_id;
            self.session_store
                .put(id, &data)
                .await
                .expect("test_set_last_response_id: put failed");
        }
    }

    pub fn test_prompt_timeout(&self) -> Duration {
        self.prompt_timeout
    }

    pub fn test_max_history_messages(&self) -> usize {
        self.max_history_messages
    }

    pub fn test_max_turns(&self) -> Option<u32> {
        self.max_turns
    }

    pub async fn test_cancel_channels_len(&self) -> usize {
        self.cancel_channels.lock().await.len()
    }

    /// Override `max_history_messages` at runtime. For tests that need a small
    /// limit without setting `XAI_MAX_HISTORY_MESSAGES` (which is unsafe in
    /// parallel tests due to env-var races).
    pub fn with_max_history_messages(mut self, n: usize) -> Self {
        self.max_history_messages = n;
        self
    }
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
        Message {
            role: role.to_string(),
            content: Some(content.to_string()),
        }
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
        // max=3, 4 messages → excess=1 → remove one pair → 2 remain (not 3)
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
        // [user_orphan, user_new, assistant_new], max=2
        // → remove orphan → [user_new, assistant_new]
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
        // [orphan1, orphan2, user, assistant], max=2
        // → remove orphan1 → remove orphan2 → [user, assistant]
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
        // assistant at front (malformed) — do not touch
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
}
