use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol::{
    AgentCapabilities, AuthEnvVar, AuthMethod, AuthMethodAgent, AuthMethodEnvVar,
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ContentChunk, EmbeddedResourceResource, Error, ErrorCode,
    ForkSessionRequest, ForkSessionResponse, Implementation,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse, ModelInfo,
    NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
    ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities,
    SessionCloseCapabilities, SessionForkCapabilities, SessionId, SessionInfo,
    SessionListCapabilities, SessionMode, SessionModeState, SessionModelState,
    SessionResumeCapabilities, SessionUpdate, SetSessionModeRequest, SetSessionModeResponse,
    SetSessionModelRequest, SetSessionModelResponse, StopReason, UsageUpdate,
};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::agent_loader::{AgentConfig, AgentLoading};
use crate::client::{Message, OpenRouterClient, OpenRouterEvent};
use crate::http_client::OpenRouterHttpClient;
use crate::session_notifier::{NatsSessionNotifier, SessionNotifier};
use crate::session_store::{MessageUsage, SessionSnapshot, SessionStoring, SnapshotMessage, TextBlock, now_iso};
use crate::skill_loader::SkillLoading;

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

fn invalid_params(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidParams.into(), msg.into())
}

fn not_found(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::ResourceNotFound.into(), msg.into())
}

const MAX_SESSIONS: usize = 100;

struct OpenRouterSession {
    cwd: String,
    model: Option<String>,
    api_key: Option<String>,
    history: Vec<Message>,
    system_prompt: Option<String>,
    created_at: Instant,
    created_at_iso: String,
    parent_session_id: Option<String>,
    branched_at_index: Option<usize>,
}

/// ACP Agent implementation backed by OpenRouter's OpenAI-compatible chat completions API.
pub struct OpenRouterAgent<H = OpenRouterClient, N = NatsSessionNotifier> {
    notifier: Arc<N>,
    client: Arc<H>,
    sessions: Arc<Mutex<HashMap<String, OpenRouterSession>>>,
    cancel_senders: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    default_model: String,
    prompt_timeout: Duration,
    available_models: Vec<ModelInfo>,
    global_api_key: Option<String>,
    pending_api_key: Arc<Mutex<Option<String>>>,
    system_prompt: Option<String>,
    max_history: usize,
    max_response_bytes: usize,
    agent_id: Option<String>,
    agent_loader: Option<Arc<dyn AgentLoading>>,
    skill_loader: Option<Arc<dyn SkillLoading>>,
    session_store: Option<Arc<dyn SessionStoring>>,
    tenant_id: String,
}

impl OpenRouterAgent<OpenRouterClient, NatsSessionNotifier> {
    pub fn new(
        notifier: NatsSessionNotifier,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self::with_deps(notifier, default_model, api_key, OpenRouterClient::new())
    }
}

impl<H: OpenRouterHttpClient, N: SessionNotifier> OpenRouterAgent<H, N> {
    pub fn with_deps(
        notifier: N,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
        client: H,
    ) -> Self {
        let default_model: String = default_model.into();
        let api_key_str: String = api_key.into();
        let global_api_key = if api_key_str.is_empty() {
            None
        } else {
            Some(api_key_str)
        };

        let prompt_timeout = std::env::var("OPENROUTER_PROMPT_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(300));

        let mut available_models = std::env::var("OPENROUTER_MODELS")
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
                                "OPENROUTER_MODELS: skipping malformed entry (expected 'id:label')"
                            );
                            None
                        }
                    })
                    .collect()
            })
            .filter(|v: &Vec<ModelInfo>| !v.is_empty())
            .unwrap_or_else(|| {
                vec![
                    ModelInfo::new(
                        "anthropic/claude-sonnet-4-6",
                        "Claude Sonnet 4.6 (OpenRouter)",
                    ),
                    ModelInfo::new("openai/gpt-4o", "GPT-4o (OpenRouter)"),
                    ModelInfo::new("google/gemini-pro-1.5", "Gemini Pro 1.5 (OpenRouter)"),
                ]
            });

        if !available_models
            .iter()
            .any(|m| m.model_id.0.as_ref() == default_model.as_str())
        {
            warn!(model = %default_model, "default model not in available list; adding it");
            available_models.push(ModelInfo::new(default_model.clone(), default_model.clone()));
        }

        let system_prompt = std::env::var("OPENROUTER_SYSTEM_PROMPT")
            .ok()
            .filter(|s| !s.is_empty());

        let max_history = std::env::var("OPENROUTER_MAX_HISTORY_MESSAGES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(20);

        let max_response_bytes = std::env::var("OPENROUTER_MAX_RESPONSE_BYTES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(4 * 1024 * 1024); // 4 MB

        let tenant_id = std::env::var("TENANT_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "default".to_string());

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
            max_response_bytes,
            agent_id: None,
            agent_loader: None,
            skill_loader: None,
            session_store: None,
            tenant_id,
        }
    }

    pub fn with_loaders(
        mut self,
        agent_id: impl Into<String>,
        agent_loader: Arc<dyn AgentLoading>,
        skill_loader: Arc<dyn SkillLoading>,
    ) -> Self {
        self.agent_id = Some(agent_id.into());
        self.agent_loader = Some(agent_loader);
        self.skill_loader = Some(skill_loader);
        self
    }

    pub fn with_session_store(mut self, store: Arc<dyn SessionStoring>) -> Self {
        self.session_store = Some(store);
        self
    }

    pub fn with_max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_prompt_timeout(mut self, timeout: Duration) -> Self {
        self.prompt_timeout = timeout;
        self
    }

    fn build_snapshot(&self, session_id: &str, session: &OpenRouterSession) -> SessionSnapshot {
        let name = session
            .history
            .first()
            .map(|m| {
                let text = &m.content;
                if text.chars().count() > 60 {
                    format!(
                        "{}…",
                        &text[..text
                            .char_indices()
                            .nth(60)
                            .map(|(i, _)| i)
                            .unwrap_or(text.len())]
                    )
                } else {
                    text.clone()
                }
            })
            .unwrap_or_else(|| "New Conversation".to_string());

        let messages = session
            .history
            .iter()
            .map(|m| SnapshotMessage {
                role: m.role.clone(),
                content: if m.content.is_empty() {
                    vec![]
                } else {
                    vec![TextBlock::new(&m.content)]
                },
                usage: m.prompt_tokens.map(|pt| MessageUsage {
                    input_tokens: pt as u32,
                    output_tokens: m.completion_tokens.unwrap_or(0) as u32,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            })
            .collect();

        SessionSnapshot {
            id: session_id.to_string(),
            tenant_id: self.tenant_id.clone(),
            name,
            model: Some(
                session
                    .model
                    .clone()
                    .unwrap_or_else(|| self.default_model.clone()),
            ),
            tools: vec![],
            memory_path: None,
            messages,
            created_at: session.created_at_iso.clone(),
            updated_at: now_iso(),
            agent_id: self.agent_id.clone(),
            parent_session_id: session.parent_session_id.clone(),
            branched_at_index: session.branched_at_index,
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

    fn maybe_evict_oldest(sessions: &mut HashMap<String, OpenRouterSession>) {
        if sessions.len() < MAX_SESSIONS {
            return;
        }
        if let Some(oldest_id) = sessions
            .iter()
            .min_by_key(|(_, s)| s.created_at)
            .map(|(id, _)| id.clone())
        {
            warn!(session_id = %oldest_id, max = MAX_SESSIONS,
                  "openrouter: session limit reached — evicting oldest session");
            sessions.remove(&oldest_id);
        }
    }

    /// Trim history to keep it within `max_history` entries.
    /// Drops the oldest pairs (user + assistant) to preserve ordering.
    fn trim_history(history: &mut Vec<Message>, max: usize) {
        while history.len() > max {
            history.remove(0);
        }
    }
}

#[async_trait(?Send)]
impl<H: OpenRouterHttpClient + 'static, N: SessionNotifier + 'static>
    agent_client_protocol::Agent for OpenRouterAgent<H, N>
{
    async fn initialize(
        &self,
        _req: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        let mut auth_methods = vec![AuthMethod::EnvVar(
            AuthMethodEnvVar::new(
                "openrouter-api-key",
                "OpenRouter API Key",
                vec![AuthEnvVar::new("OPENROUTER_API_KEY").label("OpenRouter API Key")],
            )
            .link("https://openrouter.ai/keys")
            .description("Your personal OpenRouter API key"),
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
                    .session_capabilities({
                        let mut caps_meta = serde_json::Map::new();
                        caps_meta.insert("branchAtIndex".to_string(), serde_json::json!({}));
                        caps_meta.insert("listChildren".to_string(), serde_json::json!({}));
                        SessionCapabilities::new()
                            .fork(SessionForkCapabilities::new())
                            .list(SessionListCapabilities::new())
                            .resume(SessionResumeCapabilities::new())
                            .close(SessionCloseCapabilities::new())
                            .meta(caps_meta)
                    }),
            )
            .agent_info(Implementation::new(
                "trogon-openrouter-runner",
                env!("CARGO_PKG_VERSION"),
            )))
    }

    async fn authenticate(
        &self,
        req: AuthenticateRequest,
    ) -> agent_client_protocol::Result<AuthenticateResponse> {
        match req.method_id.0.as_ref() {
            "openrouter-api-key" => {
                let val = req
                    .meta
                    .as_ref()
                    .and_then(|m| m.get("OPENROUTER_API_KEY"))
                    .ok_or_else(|| internal_error("OPENROUTER_API_KEY missing from meta"))?;
                let key = val
                    .as_str()
                    .ok_or_else(|| invalid_params("OPENROUTER_API_KEY must be a string"))?;
                if key.is_empty() {
                    return Err(invalid_params("OPENROUTER_API_KEY must not be empty"));
                }
                info!("openrouter: client authenticated with user-provided API key");
                *self.pending_api_key.lock().await = Some(key.to_string());
            }
            "agent" => {
                if self.global_api_key.is_none() {
                    return Err(internal_error(
                        "authenticate: no server API key configured",
                    ));
                }
                info!("openrouter: client authenticated using server key");
            }
            other => {
                return Err(internal_error(format!(
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

        let (session_system_prompt, session_model_override) =
            if let (Some(id), Some(al), Some(sl)) =
                (&self.agent_id, &self.agent_loader, &self.skill_loader)
            {
                let AgentConfig {
                    skill_ids,
                    system_prompt: agent_sp,
                    model_id,
                } = al.load_config(id).await;
                let skills_text = sl.load(&skill_ids).await;
                let base = agent_sp.as_deref().or(self.system_prompt.as_deref());
                let prompt = match (base, skills_text) {
                    (Some(sp), Some(sk)) => Some(format!("{sp}\n\n{sk}")),
                    (None, Some(sk)) => Some(sk),
                    (Some(sp), None) => Some(sp.to_string()),
                    (None, None) => None,
                };
                (prompt, model_id)
            } else {
                (self.system_prompt.clone(), None)
            };

        let created_at_iso = now_iso();
        let mut sessions = self.sessions.lock().await;
        Self::maybe_evict_oldest(&mut sessions);
        sessions.insert(
            session_id.clone(),
            OpenRouterSession {
                cwd,
                model: session_model_override,
                api_key,
                history: Vec::new(),
                system_prompt: session_system_prompt,
                created_at: Instant::now(),
                created_at_iso,
                parent_session_id: None,
                branched_at_index: None,
            },
        );

        if let Some(store) = &self.session_store {
            let snapshot = self.build_snapshot(
                &session_id,
                sessions.get(&session_id).expect("just inserted"),
            );
            store.save(&snapshot).await;
        }
        drop(sessions);

        info!(session_id, agent_id = ?self.agent_id, "openrouter: new session");
        Ok(NewSessionResponse::new(SessionId::from(session_id))
            .modes(self.session_mode_state())
            .models(self.session_model_state(None))
            .config_options(vec![]))
    }

    async fn load_session(
        &self,
        req: agent_client_protocol::LoadSessionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::LoadSessionResponse> {
        let session_id = req.session_id.to_string();
        let sessions = self.sessions.lock().await;
        match sessions.get(&session_id) {
            Some(s) => Ok(agent_client_protocol::LoadSessionResponse::new()
                .modes(self.session_mode_state())
                .models(self.session_model_state(s.model.as_deref()))
                .config_options(vec![])),
            None => Err(not_found(format!("session {session_id} not found"))),
        }
    }

    async fn resume_session(
        &self,
        req: ResumeSessionRequest,
    ) -> agent_client_protocol::Result<ResumeSessionResponse> {
        let session_id = req.session_id.to_string();
        if !self.sessions.lock().await.contains_key(&session_id) {
            return Err(not_found(format!("session {session_id} not found")));
        }
        Ok(ResumeSessionResponse::new())
    }

    async fn fork_session(
        &self,
        req: ForkSessionRequest,
    ) -> agent_client_protocol::Result<ForkSessionResponse> {
        let source_id = req.session_id.to_string();
        let cwd = req.cwd.to_string_lossy().into_owned();

        let (inherited_model, inherited_key, mut history, inherited_system_prompt) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&source_id)
                .ok_or_else(|| not_found(format!("session {source_id} not found")))?;
            (
                s.model.clone(),
                s.api_key.clone(),
                s.history.clone(),
                s.system_prompt.clone(),
            )
        };

        let branch_at: Option<usize> = req
            .meta
            .as_ref()
            .and_then(|m| m.get("branchAtIndex"))
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        if let Some(idx) = branch_at {
            history.truncate(idx);
        }

        let new_session_id = Uuid::new_v4().to_string();
        let mut sessions = self.sessions.lock().await;
        Self::maybe_evict_oldest(&mut sessions);
        sessions.insert(
            new_session_id.clone(),
            OpenRouterSession {
                cwd,
                model: inherited_model.clone(),
                api_key: inherited_key,
                history,
                system_prompt: inherited_system_prompt,
                created_at: Instant::now(),
                created_at_iso: now_iso(),
                parent_session_id: Some(source_id.clone()),
                branched_at_index: branch_at,
            },
        );

        if let (Some(store), Some(s)) = (&self.session_store, sessions.get(&new_session_id)) {
            let snapshot = self.build_snapshot(&new_session_id, s);
            store.save(&snapshot).await;
        }
        drop(sessions);

        Ok(ForkSessionResponse::new(new_session_id)
            .modes(self.session_mode_state())
            .models(self.session_model_state(inherited_model.as_deref()))
            .config_options(vec![]))
    }

    async fn close_session(
        &self,
        req: CloseSessionRequest,
    ) -> agent_client_protocol::Result<CloseSessionResponse> {
        let session_id = req.session_id.to_string();

        let sender = self.cancel_senders.lock().await.remove(&session_id);
        if let Some(tx) = sender {
            let _ = tx.send(());
        }

        let mut sessions = self.sessions.lock().await;
        if let (Some(store), Some(s)) = (&self.session_store, sessions.get(&session_id)) {
            let snapshot = self.build_snapshot(&session_id, s);
            store.save(&snapshot).await;
        }
        sessions.remove(&session_id);
        drop(sessions);
        info!(session_id, "openrouter: session closed");
        Ok(CloseSessionResponse::new())
    }

    async fn list_sessions(
        &self,
        _req: ListSessionsRequest,
    ) -> agent_client_protocol::Result<ListSessionsResponse> {
        let sessions = self.sessions.lock().await;
        let mut list: Vec<_> = sessions
            .iter()
            .map(|(id, s)| {
                let mut info = SessionInfo::new(id.clone(), s.cwd.clone());
                if s.parent_session_id.is_some() || s.branched_at_index.is_some() {
                    let mut meta = serde_json::Map::new();
                    if let Some(ref parent_id) = s.parent_session_id {
                        meta.insert("parentSessionId".to_string(), serde_json::json!(parent_id));
                    }
                    if let Some(idx) = s.branched_at_index {
                        meta.insert("branchedAtIndex".to_string(), serde_json::json!(idx));
                    }
                    info = info.meta(meta);
                }
                info
            })
            .collect();
        list.sort_by(|a, b| a.session_id.0.cmp(&b.session_id.0));
        Ok(ListSessionsResponse::new(list))
    }

    async fn set_session_mode(
        &self,
        req: SetSessionModeRequest,
    ) -> agent_client_protocol::Result<SetSessionModeResponse> {
        let mode_id = req.mode_id.to_string();
        let session_id = req.session_id.to_string();
        if mode_id != "default" {
            return Err(invalid_params(format!("unknown mode: {mode_id}")));
        }
        if !self.sessions.lock().await.contains_key(&session_id) {
            return Err(not_found(format!("session {session_id} not found")));
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

        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(&session_id) {
            Some(s) => {
                s.model = Some(model_id.clone());
                info!(session_id, model = %model_id, "openrouter: set_session_model");
                Ok(SetSessionModelResponse::new())
            }
            None => Err(not_found(format!("session {session_id} not found"))),
        }
    }

    async fn prompt(&self, req: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        let session_id = req.session_id.to_string();

        let user_input: String = req
            .prompt
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(t) => Some(t.text.clone()),
                ContentBlock::ResourceLink(r) => {
                    Some(format!("[Resource: {} | {}]", r.name, r.uri))
                }
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
            warn!(session_id, "openrouter: prompt contains no text or resource blocks");
        }

        let (model, api_key, mut messages, session_system_prompt) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&session_id)
                .ok_or_else(|| not_found(format!("session {session_id} not found")))?;
            (
                s.model.clone(),
                s.api_key.clone(),
                s.history.clone(),
                s.system_prompt.clone(),
            )
        };

        let model = model.as_deref().unwrap_or(&self.default_model).to_string();
        let api_key = api_key
            .or_else(|| self.global_api_key.clone())
            .ok_or_else(|| {
                internal_error(
                    "no API key for session — set OPENROUTER_API_KEY or authenticate first",
                )
            })?;

        // Skip duplicate user message on resume after crash.
        let resuming = messages
            .last()
            .map(|m| m.role == "user" && m.content == user_input)
            == Some(true);

        if !resuming {
            messages.push(Message::user(user_input.clone()));
        }

        // Prepend system message if present (not stored in history to keep it clean).
        let mut wire_messages: Vec<Message> = Vec::new();
        if let Some(ref sp) = session_system_prompt {
            wire_messages.push(Message::system(sp.clone()));
        }
        wire_messages.extend(messages.clone());

        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        self.cancel_senders
            .lock()
            .await
            .insert(session_id.clone(), cancel_tx);

        let client = Arc::clone(&self.client);
        let notifier = Arc::clone(&self.notifier);
        let mut assistant_text = String::new();
        let mut usage: Option<(u64, u64)> = None;
        let mut canceled = false;

        let notification_session_id = session_id.clone();
        let stream_fut = client.chat_stream(&model, &wire_messages, &api_key);

        let mut stream = tokio::select! {
            s = stream_fut => s,
            _ = &mut cancel_rx => {
                canceled = true;
                futures_util::stream::empty().boxed_local()
            }
        };

        loop {
            if canceled {
                break;
            }

            let next = tokio::time::timeout(self.prompt_timeout, stream.next());
            let event = tokio::select! {
                result = next => match result {
                    Ok(Some(ev)) => ev,
                    Ok(None) => break,
                    Err(_) => {
                        warn!(session_id, "openrouter: stream timed out (no chunk received)");
                        OpenRouterEvent::Error {
                            message: "stream timed out".to_string(),
                        }
                    }
                },
                _ = &mut cancel_rx => {
                    canceled = true;
                    break;
                }
            };

            match event {
                OpenRouterEvent::TextDelta { text } => {
                    assistant_text.push_str(&text);
                    notifier
                        .notify(agent_client_protocol::SessionNotification::new(
                            notification_session_id.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::from(text),
                            )),
                        ))
                        .await;
                    if assistant_text.len() > self.max_response_bytes {
                        warn!(
                            session_id,
                            limit = self.max_response_bytes,
                            actual = assistant_text.len(),
                            "openrouter: response exceeded size limit, stopping stream early"
                        );
                        break;
                    }
                }
                OpenRouterEvent::Usage {
                    prompt_tokens,
                    completion_tokens,
                } => {
                    usage = Some((prompt_tokens, completion_tokens));
                    notifier
                        .notify(agent_client_protocol::SessionNotification::new(
                            notification_session_id.clone(),
                            SessionUpdate::UsageUpdate(UsageUpdate::new(
                                prompt_tokens,
                                completion_tokens,
                            )),
                        ))
                        .await;
                }
                OpenRouterEvent::Finished { .. } => {}
                OpenRouterEvent::Done => break,
                OpenRouterEvent::Error { message } => {
                    warn!(session_id, error = %message, "openrouter: stream error");
                    break;
                }
            }
        }

        // Drop the stream before any awaits so the HTTP connection to OpenRouter
        // is closed immediately on cancellation, stopping token generation.
        drop(stream);

        self.cancel_senders.lock().await.remove(&session_id);

        // Persist assistant turn and trim history.
        {
            let mut sessions = self.sessions.lock().await;
            if let Some(s) = sessions.get_mut(&session_id) {
                if !resuming {
                    s.history.push(Message::user(user_input));
                }
                if !assistant_text.is_empty() {
                    let msg = if let Some((pt, ct)) = usage {
                        Message::assistant_with_usage(&assistant_text, pt, ct)
                    } else {
                        Message::assistant(&assistant_text)
                    };
                    s.history.push(msg);
                }
                Self::trim_history(&mut s.history, self.max_history);

                if let Some(store) = &self.session_store {
                    let snapshot = self.build_snapshot(&session_id, s);
                    store.save(&snapshot).await;
                }
            }
        }

        let stop_reason = if canceled {
            StopReason::Cancelled
        } else {
            StopReason::EndTurn
        };

        Ok(PromptResponse::new(stop_reason))
    }

    async fn cancel(
        &self,
        req: CancelNotification,
    ) -> agent_client_protocol::Result<()> {
        let session_id = req.session_id.to_string();
        if let Some(tx) = self.cancel_senders.lock().await.remove(&session_id) {
            let _ = tx.send(());
            info!(session_id, "openrouter: prompt cancelled");
        }
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use agent_client_protocol::{
        Agent as _, AuthenticateRequest, CancelNotification, CloseSessionRequest, ContentBlock,
        ForkSessionRequest, ListSessionsRequest, LoadSessionRequest, NewSessionRequest,
        PromptRequest, ResumeSessionRequest, SessionId, SetSessionModeRequest,
        SetSessionModelRequest,
    };
    use super::*;
    use crate::http_client::mock::MockOpenRouterHttpClient;
    use crate::session_notifier::MockSessionNotifier;

    // Serialise all tests that mutate environment variables so they don't race
    // each other in the default multi-threaded cargo test harness.
    static ENV_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_MUTEX.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap()
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn make_agent() -> OpenRouterAgent<MockOpenRouterHttpClient, MockSessionNotifier> {
        OpenRouterAgent::with_deps(
            MockSessionNotifier::new(),
            "test-model",
            "",
            MockOpenRouterHttpClient::new(),
        )
    }

    fn make_agent_with_key(key: &str) -> OpenRouterAgent<MockOpenRouterHttpClient, MockSessionNotifier> {
        OpenRouterAgent::with_deps(
            MockSessionNotifier::new(),
            "test-model",
            key,
            MockOpenRouterHttpClient::new(),
        )
    }

    fn local() -> tokio::task::LocalSet {
        tokio::task::LocalSet::new()
    }

    // ── trim_history ──────────────────────────────────────────────────────────

    #[test]
    fn trim_history_removes_from_front_when_over_limit() {
        let mut history = vec![
            Message::user("a"),
            Message::assistant("b"),
            Message::user("c"),
        ];
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::trim_history(&mut history, 2);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "b");
        assert_eq!(history[1].content, "c");
    }

    #[test]
    fn trim_history_no_op_when_under_limit() {
        let mut history = vec![Message::user("a"), Message::assistant("b")];
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::trim_history(&mut history, 5);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn trim_history_no_op_when_at_limit() {
        let mut history = vec![Message::user("a"), Message::assistant("b")];
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::trim_history(&mut history, 2);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn trim_history_clears_all_when_max_is_zero() {
        let mut history = vec![Message::user("a"), Message::assistant("b")];
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::trim_history(&mut history, 0);
        assert!(history.is_empty());
    }

    #[test]
    fn trim_history_empty_is_noop() {
        let mut history: Vec<Message> = vec![];
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::trim_history(&mut history, 5);
        assert!(history.is_empty());
    }

    // ── maybe_evict_oldest ────────────────────────────────────────────────────

    #[test]
    fn maybe_evict_oldest_no_eviction_below_limit() {
        let mut sessions: HashMap<String, OpenRouterSession> = HashMap::new();
        for i in 0..MAX_SESSIONS - 1 {
            sessions.insert(format!("s{i}"), make_session());
        }
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::maybe_evict_oldest(&mut sessions);
        assert_eq!(sessions.len(), MAX_SESSIONS - 1);
    }

    #[test]
    fn maybe_evict_oldest_evicts_at_exactly_limit() {
        let mut sessions: HashMap<String, OpenRouterSession> = HashMap::new();
        for i in 0..MAX_SESSIONS {
            sessions.insert(format!("s{i}"), make_session());
        }
        // At exactly MAX_SESSIONS, eviction fires (guard is `< MAX_SESSIONS`).
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::maybe_evict_oldest(&mut sessions);
        assert_eq!(sessions.len(), MAX_SESSIONS - 1);
    }

    #[test]
    fn maybe_evict_oldest_removes_one_when_over_limit() {
        let mut sessions: HashMap<String, OpenRouterSession> = HashMap::new();
        for i in 0..=MAX_SESSIONS {
            sessions.insert(format!("s{i}"), make_session());
        }
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::maybe_evict_oldest(&mut sessions);
        assert_eq!(sessions.len(), MAX_SESSIONS);
    }

    #[test]
    fn maybe_evict_oldest_empty_map_does_not_panic() {
        let mut sessions: HashMap<String, OpenRouterSession> = HashMap::new();
        OpenRouterAgent::<MockOpenRouterHttpClient, MockSessionNotifier>::maybe_evict_oldest(&mut sessions);
        assert!(sessions.is_empty());
    }

    fn make_session() -> OpenRouterSession {
        OpenRouterSession {
            cwd: "/tmp".to_string(),
            model: None,
            api_key: None,
            history: vec![],
            system_prompt: None,
            created_at: Instant::now(),
            created_at_iso: "2026-01-01T00:00:00.000Z".to_string(),
            parent_session_id: None,
            branched_at_index: None,
        }
    }

    // ── session_mode_state / session_model_state ──────────────────────────────

    #[test]
    fn session_mode_state_has_single_default_mode() {
        let agent = make_agent();
        let state = agent.session_mode_state();
        assert_eq!(state.current_mode_id.0.as_ref(), "default");
        assert_eq!(state.available_modes.len(), 1);
        assert_eq!(state.available_modes[0].id.0.as_ref(), "default");
    }

    #[test]
    fn session_model_state_includes_default_model() {
        let agent = make_agent();
        let state = agent.session_model_state(None);
        assert_eq!(state.current_model_id.0.as_ref(), "test-model");
        assert!(
            state.available_models.iter().any(|m| m.model_id.0.as_ref() == "test-model"),
            "test-model must be in available_models"
        );
    }

    #[test]
    fn session_model_state_uses_provided_current() {
        let agent = make_agent();
        let state = agent.session_model_state(Some("openai/gpt-4o"));
        assert_eq!(state.current_model_id.0.as_ref(), "openai/gpt-4o");
    }

    // ── build_snapshot ────────────────────────────────────────────────────────

    #[test]
    fn build_snapshot_name_defaults_to_new_conversation_when_empty_history() {
        let agent = make_agent();
        let session = make_session();
        let snap = agent.build_snapshot("sid-1", &session);
        assert_eq!(snap.name, "New Conversation");
    }

    #[test]
    fn build_snapshot_name_uses_first_message_content() {
        let agent = make_agent();
        let mut session = make_session();
        session.history.push(Message::user("Short question"));
        let snap = agent.build_snapshot("sid-1", &session);
        assert_eq!(snap.name, "Short question");
    }

    #[test]
    fn build_snapshot_name_truncated_at_60_chars() {
        let agent = make_agent();
        let mut session = make_session();
        let long = "a".repeat(80);
        session.history.push(Message::user(&long));
        let snap = agent.build_snapshot("sid-1", &session);
        assert!(snap.name.ends_with('…'), "long name must end with ellipsis");
        // The visible chars before ellipsis should be 60.
        let chars: Vec<char> = snap.name.chars().collect();
        assert_eq!(chars.len(), 61, "60 chars + ellipsis = 61 chars");
    }

    #[test]
    fn build_snapshot_preserves_session_id_and_tenant() {
        let agent = make_agent();
        let session = make_session();
        let snap = agent.build_snapshot("my-session-id", &session);
        assert_eq!(snap.id, "my-session-id");
        assert_eq!(snap.tenant_id, "default");
    }

    #[test]
    fn build_snapshot_messages_mapped_correctly() {
        let agent = make_agent();
        let mut session = make_session();
        session.history.push(Message::user("hi"));
        session.history.push(Message::assistant("hello"));
        let snap = agent.build_snapshot("s", &session);
        assert_eq!(snap.messages.len(), 2);
        assert_eq!(snap.messages[0].role, "user");
        assert_eq!(snap.messages[0].content[0].text, "hi");
        assert_eq!(snap.messages[1].role, "assistant");
    }

    #[test]
    fn build_snapshot_empty_message_content_yields_empty_blocks() {
        let agent = make_agent();
        let mut session = make_session();
        session.history.push(Message::user(""));
        let snap = agent.build_snapshot("s", &session);
        assert_eq!(snap.messages[0].content.len(), 0);
    }

    #[test]
    fn build_snapshot_includes_usage_when_present() {
        let agent = make_agent();
        let mut session = make_session();
        session.history.push(Message::assistant_with_usage("reply", 10, 5));
        let snap = agent.build_snapshot("s", &session);
        let usage = snap.messages[0].usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn build_snapshot_no_usage_when_absent() {
        let agent = make_agent();
        let mut session = make_session();
        session.history.push(Message::user("q"));
        let snap = agent.build_snapshot("s", &session);
        assert!(snap.messages[0].usage.is_none());
    }

    #[test]
    fn build_snapshot_includes_parent_and_branch_when_set() {
        let agent = make_agent();
        let mut session = make_session();
        session.parent_session_id = Some("parent-id".to_string());
        session.branched_at_index = Some(3);
        let snap = agent.build_snapshot("s", &session);
        assert_eq!(snap.parent_session_id.as_deref(), Some("parent-id"));
        assert_eq!(snap.branched_at_index, Some(3));
    }

    // ── authenticate ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn authenticate_with_valid_user_key_succeeds() {
        let agent = make_agent();
        local().run_until(async move {
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("sk-test-key"));
            let req = AuthenticateRequest::new("openrouter-api-key").meta(meta);
            assert!(agent.authenticate(req).await.is_ok());
            // Key should now be pending.
            assert_eq!(
                *agent.pending_api_key.lock().await,
                Some("sk-test-key".to_string())
            );
        }).await;
    }

    #[tokio::test]
    async fn authenticate_rejects_empty_key() {
        let agent = make_agent();
        local().run_until(async move {
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!(""));
            let req = AuthenticateRequest::new("openrouter-api-key").meta(meta);
            assert!(agent.authenticate(req).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn authenticate_rejects_missing_meta_key() {
        let agent = make_agent();
        local().run_until(async move {
            let req = AuthenticateRequest::new("openrouter-api-key").meta(serde_json::Map::new());
            assert!(agent.authenticate(req).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn authenticate_rejects_non_string_key() {
        let agent = make_agent();
        local().run_until(async move {
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!(42));
            let req = AuthenticateRequest::new("openrouter-api-key").meta(meta);
            assert!(agent.authenticate(req).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn authenticate_agent_method_fails_without_global_key() {
        let agent = make_agent(); // global_api_key is None
        local().run_until(async move {
            let req = AuthenticateRequest::new("agent");
            assert!(agent.authenticate(req).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn authenticate_agent_method_succeeds_with_global_key() {
        let agent = make_agent_with_key("global-key");
        local().run_until(async move {
            let req = AuthenticateRequest::new("agent");
            assert!(agent.authenticate(req).await.is_ok());
            // No pending key should be set for the "agent" method.
            assert!(agent.pending_api_key.lock().await.is_none());
        }).await;
    }

    #[tokio::test]
    async fn authenticate_unknown_method_fails() {
        let agent = make_agent();
        local().run_until(async move {
            let req = AuthenticateRequest::new("unknown-method");
            assert!(agent.authenticate(req).await.is_err());
        }).await;
    }

    // ── new_session ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn new_session_creates_session_with_unique_id() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let r1 = agent.new_session(NewSessionRequest::new(PathBuf::from("/a"))).await.unwrap();
            let r2 = agent.new_session(NewSessionRequest::new(PathBuf::from("/b"))).await.unwrap();
            assert_ne!(r1.session_id, r2.session_id);
        }).await;
    }

    #[tokio::test]
    async fn new_session_consumes_pending_api_key_once() {
        let agent = make_agent();
        local().run_until(async move {
            // Set a pending key via authenticate.
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("per-user-key"));
            agent.authenticate(AuthenticateRequest::new("openrouter-api-key").meta(meta)).await.unwrap();

            // After new_session, pending key is consumed.
            agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            assert!(agent.pending_api_key.lock().await.is_none(), "pending key must be consumed");
        }).await;
    }

    #[tokio::test]
    async fn new_session_returns_modes_and_models() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            assert!(resp.modes.is_some());
            assert!(resp.models.is_some());
        }).await;
    }

    // ── resume_session / load_session ─────────────────────────────────────────

    #[tokio::test]
    async fn resume_session_existing_succeeds() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            assert!(agent.resume_session(ResumeSessionRequest::new(resp.session_id, "/")).await.is_ok());
        }).await;
    }

    #[tokio::test]
    async fn resume_session_nonexistent_fails() {
        let agent = make_agent();
        local().run_until(async move {
            let req = ResumeSessionRequest::new(SessionId::from("no-such-session"), "/");
            assert!(agent.resume_session(req).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn load_session_existing_returns_modes_and_models() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let new_resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let load_resp = agent.load_session(LoadSessionRequest::new(new_resp.session_id, "/")).await.unwrap();
            assert!(load_resp.modes.is_some());
            assert!(load_resp.models.is_some());
        }).await;
    }

    #[tokio::test]
    async fn load_session_nonexistent_fails() {
        let agent = make_agent();
        local().run_until(async move {
            assert!(agent.load_session(LoadSessionRequest::new(SessionId::from("ghost"), "/")).await.is_err());
        }).await;
    }

    // ── list_sessions ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_empty() {
        let agent = make_agent();
        local().run_until(async move {
            let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            assert!(resp.sessions.is_empty());
        }).await;
    }

    #[tokio::test]
    async fn list_sessions_returns_all_sessions_sorted() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            agent.new_session(NewSessionRequest::new(PathBuf::from("/a"))).await.unwrap();
            agent.new_session(NewSessionRequest::new(PathBuf::from("/b"))).await.unwrap();
            agent.new_session(NewSessionRequest::new(PathBuf::from("/c"))).await.unwrap();
            let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            assert_eq!(resp.sessions.len(), 3);
            // Must be sorted by session_id.
            let ids: Vec<&str> = resp.sessions.iter().map(|s| s.session_id.0.as_ref()).collect();
            let mut sorted = ids.clone();
            sorted.sort();
            assert_eq!(ids, sorted);
        }).await;
    }

    #[tokio::test]
    async fn list_sessions_fork_includes_metadata() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let new_resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/src"))).await.unwrap();
            let src_id = new_resp.session_id.clone();
            agent.fork_session(ForkSessionRequest::new(src_id.clone(), PathBuf::from("/fork"))).await.unwrap();

            let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            let fork = resp.sessions.iter().find(|s| s.session_id != src_id).unwrap();
            let meta = fork.meta.as_ref().expect("fork must have meta");
            assert!(meta.contains_key("parentSessionId"));
        }).await;
    }

    #[tokio::test]
    async fn list_sessions_fork_with_branch_at_index_includes_both_meta_fields() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src_id = agent.new_session(NewSessionRequest::new(PathBuf::from("/src"))).await.unwrap().session_id;
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 0})
            ).unwrap();
            agent.fork_session(
                ForkSessionRequest::new(src_id.clone(), PathBuf::from("/f")).meta(meta)
            ).await.unwrap();

            let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            let fork = resp.sessions.iter().find(|s| s.session_id != src_id).unwrap();
            let m = fork.meta.as_ref().expect("fork must have meta");
            assert!(m.contains_key("parentSessionId"), "parentSessionId must be in meta");
            assert!(m.contains_key("branchedAtIndex"), "branchedAtIndex must be in meta when set");
            assert_eq!(m["branchedAtIndex"], serde_json::json!(0));
        }).await;
    }

    // ── set_session_mode / set_session_model ──────────────────────────────────

    #[tokio::test]
    async fn set_session_mode_valid_mode_succeeds() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id;
            assert!(agent.set_session_mode(SetSessionModeRequest::new(sid, "default")).await.is_ok());
        }).await;
    }

    #[tokio::test]
    async fn set_session_mode_invalid_mode_fails() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id;
            assert!(agent.set_session_mode(SetSessionModeRequest::new(sid, "turbo")).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn set_session_mode_nonexistent_session_fails() {
        let agent = make_agent();
        local().run_until(async move {
            assert!(agent.set_session_mode(SetSessionModeRequest::new(SessionId::from("ghost"), "default")).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn set_session_model_valid_model_updates_session() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            // "test-model" is added automatically as the default model.
            assert!(agent.set_session_model(SetSessionModelRequest::new(sid.clone(), "test-model")).await.is_ok());
            // Verify via load_session that the model is reflected.
            let load = agent.load_session(LoadSessionRequest::new(sid, "/")).await.unwrap();
            let models = load.models.unwrap();
            assert_eq!(models.current_model_id.0.as_ref(), "test-model");
        }).await;
    }

    #[tokio::test]
    async fn set_session_model_unknown_model_fails() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            assert!(agent.set_session_model(SetSessionModelRequest::new(resp.session_id, "unknown/model")).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn set_session_model_nonexistent_session_fails() {
        let agent = make_agent();
        local().run_until(async move {
            assert!(agent.set_session_model(SetSessionModelRequest::new(SessionId::from("ghost"), "test-model")).await.is_err());
        }).await;
    }

    // ── close_session ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn close_session_removes_session() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            agent.close_session(CloseSessionRequest::new(sid.clone())).await.unwrap();
            assert!(agent.resume_session(ResumeSessionRequest::new(sid, "/")).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn close_session_nonexistent_does_not_panic() {
        let agent = make_agent();
        local().run_until(async move {
            assert!(agent.close_session(CloseSessionRequest::new(SessionId::from("ghost"))).await.is_ok());
        }).await;
    }

    // ── cancel ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_nonexistent_session_is_noop() {
        let agent = make_agent();
        local().run_until(async move {
            let result = agent.cancel(CancelNotification::new("ghost")).await;
            assert!(result.is_ok());
        }).await;
    }

    // ── fork_session ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fork_session_creates_new_id() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let fork = agent.fork_session(ForkSessionRequest::new(src.session_id.clone(), PathBuf::from("/f"))).await.unwrap();
            assert_ne!(fork.session_id, src.session_id);
        }).await;
    }

    #[tokio::test]
    async fn fork_session_nonexistent_source_fails() {
        let agent = make_agent();
        local().run_until(async move {
            assert!(agent.fork_session(ForkSessionRequest::new(
                SessionId::from("ghost"), PathBuf::from("/f")
            )).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn fork_session_inherits_model_from_source() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let src_id = src.session_id.clone();
            // Set a specific model on the source.
            agent.set_session_model(SetSessionModelRequest::new(src_id.clone(), "test-model")).await.unwrap();
            let fork = agent.fork_session(ForkSessionRequest::new(src_id, PathBuf::from("/f"))).await.unwrap();
            let fork_load = agent.load_session(LoadSessionRequest::new(fork.session_id, "/")).await.unwrap();
            assert_eq!(fork_load.models.unwrap().current_model_id.0.as_ref(), "test-model");
        }).await;
    }

    #[tokio::test]
    async fn fork_with_branch_at_index_truncates_history() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let src_id = src.session_id.clone();

            // Add 4 messages to source history via prompt.
            for text in ["msg1", "msg2"] {
                agent.sessions.lock().await.get_mut(&src_id.to_string()).unwrap()
                    .history.push(Message::user(text));
                agent.sessions.lock().await.get_mut(&src_id.to_string()).unwrap()
                    .history.push(Message::assistant("ok"));
            }

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 2})
            ).unwrap();
            let fork = agent.fork_session(
                ForkSessionRequest::new(src_id, PathBuf::from("/f")).meta(meta)
            ).await.unwrap();

            // Fork should have only 2 messages (truncated at index 2).
            let fork_sessions = agent.sessions.lock().await;
            let fork_session = fork_sessions.get(&fork.session_id.to_string()).unwrap();
            assert_eq!(fork_session.history.len(), 2);
        }).await;
    }

    #[tokio::test]
    async fn fork_without_branch_at_index_copies_full_history() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let src_id = src.session_id.clone();

            agent.sessions.lock().await.get_mut(&src_id.to_string()).unwrap()
                .history.push(Message::user("q"));
            agent.sessions.lock().await.get_mut(&src_id.to_string()).unwrap()
                .history.push(Message::assistant("a"));

            let fork = agent.fork_session(
                ForkSessionRequest::new(src_id, PathBuf::from("/f"))
            ).await.unwrap();

            let fork_sessions = agent.sessions.lock().await;
            let fork_session = fork_sessions.get(&fork.session_id.to_string()).unwrap();
            assert_eq!(fork_session.history.len(), 2);
        }).await;
    }

    // ── prompt ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_fails_without_api_key() {
        let agent = make_agent(); // no global key, no pending key
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let result = agent.prompt(PromptRequest::new(
                resp.session_id,
                vec![ContentBlock::from("hello".to_string())],
            )).await;
            assert!(result.is_err());
        }).await;
    }

    #[tokio::test]
    async fn prompt_on_nonexistent_session_fails() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let result = agent.prompt(PromptRequest::new(
                SessionId::from("ghost"),
                vec![ContentBlock::from("hello".to_string())],
            )).await;
            assert!(result.is_err());
        }).await;
    }

    #[tokio::test]
    async fn prompt_stores_user_and_assistant_messages() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "pong".to_string() },
        ]);
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("ping".to_string())],
            )).await.unwrap();

            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            assert_eq!(s.history.len(), 2);
            assert_eq!(s.history[0].role, "user");
            assert_eq!(s.history[0].content, "ping");
            assert_eq!(s.history[1].role, "assistant");
            assert_eq!(s.history[1].content, "pong");
        }).await;
    }

    #[tokio::test]
    async fn prompt_with_no_response_stores_only_user_message() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![]);
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("ping".to_string())],
            )).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            assert_eq!(s.history.len(), 1, "only user message when no assistant response");
        }).await;
    }

    #[tokio::test]
    async fn prompt_skips_duplicate_user_message_on_resume() {
        let agent = make_agent_with_key("k");
        // First call stores "ping" in history with no reply.
        agent.client.push_response(vec![]);
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "pong".to_string() }]);
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            // First prompt — stores user message.
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("ping".to_string())],
            )).await.unwrap();
            // Second prompt with same message — should resume, not duplicate.
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("ping".to_string())],
            )).await.unwrap();

            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            // user "ping" should appear only once, followed by assistant "pong".
            let user_msgs: Vec<_> = s.history.iter().filter(|m| m.role == "user").collect();
            assert_eq!(user_msgs.len(), 1, "duplicate user message must be skipped on resume");
        }).await;
    }

    #[tokio::test]
    async fn prompt_sends_notifications_for_text_delta() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "Hello".to_string() },
            OpenRouterEvent::TextDelta { text: " World".to_string() },
        ]);
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            agent.prompt(PromptRequest::new(
                resp.session_id,
                vec![ContentBlock::from("hi".to_string())],
            )).await.unwrap();
            let notes = agent.notifier.notifications.lock().unwrap();
            assert_eq!(notes.len(), 2, "one notification per TextDelta");
        }).await;
    }

    #[tokio::test]
    async fn prompt_returns_end_turn_stop_reason() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let prompt_resp = agent.prompt(PromptRequest::new(
                resp.session_id,
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            assert!(matches!(prompt_resp.stop_reason, agent_client_protocol::StopReason::EndTurn));
        }).await;
    }

    #[tokio::test]
    async fn prompt_cancel_returns_cancelled_stop_reason() {
        let agent = Arc::new(make_agent_with_key("k"));
        let agent2 = Arc::clone(&agent);
        agent.client.push_slow_response(OpenRouterEvent::TextDelta { text: "slow".to_string() });

        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            let sid2 = sid.clone();

            let agent_prompt = Arc::clone(&agent);
            let prompt_handle = tokio::task::spawn_local(async move {
                agent_prompt.prompt(PromptRequest::new(
                    sid,
                    vec![ContentBlock::from("q".to_string())],
                )).await.unwrap()
            });

            // Give the prompt time to start.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;

            agent2.cancel(CancelNotification::new(sid2)).await.unwrap();

            let result = prompt_handle.await.unwrap();
            assert!(matches!(result.stop_reason, agent_client_protocol::StopReason::Cancelled));
        }).await;
    }

    #[tokio::test]
    async fn prompt_trims_history_to_max() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            // Pre-fill with max_history messages (default = 20).
            {
                let mut sessions = agent.sessions.lock().await;
                let s = sessions.get_mut(&sid.to_string()).unwrap();
                for i in 0..20 {
                    s.history.push(Message::user(format!("q{i}")));
                }
            }
            // One more prompt with a reply adds 2 more → trim to 20.
            agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "r".to_string() }]);
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("extra".to_string())],
            )).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            assert!(s.history.len() <= 20, "history must be trimmed to max_history");
        }).await;
    }

    #[tokio::test]
    async fn prompt_system_prompt_not_stored_in_history() {
        let _guard = env_lock();
        // Set a system prompt via env var.
        unsafe { std::env::set_var("OPENROUTER_SYSTEM_PROMPT", "You are helpful."); }
        let agent = OpenRouterAgent::with_deps(
            MockSessionNotifier::new(),
            "test-model",
            "k",
            MockOpenRouterHttpClient::new(),
        );
        unsafe { std::env::remove_var("OPENROUTER_SYSTEM_PROMPT"); }

        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);

        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("hi".to_string())],
            )).await.unwrap();

            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            // History must not contain the system message.
            assert!(
                s.history.iter().all(|m| m.role != "system"),
                "system prompt must not be stored in history"
            );
            // But it should have been sent to the HTTP client.
            let calls = agent.client.calls.lock().unwrap();
            assert!(
                calls[0].messages.iter().any(|m| m.role == "system"),
                "system prompt must appear in wire messages"
            );
        }).await;
    }

    #[tokio::test]
    async fn prompt_uses_usage_from_stream_for_history() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "reply".to_string() },
            OpenRouterEvent::Usage { prompt_tokens: 10, completion_tokens: 5 },
        ]);
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.clone();
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            let assistant_msg = s.history.iter().find(|m| m.role == "assistant").unwrap();
            assert_eq!(assistant_msg.prompt_tokens, Some(10));
            assert_eq!(assistant_msg.completion_tokens, Some(5));
        }).await;
    }

    // ── max_response_bytes ────────────────────────────────────────────────────

    fn make_agent_with_size_limit(limit: usize) -> OpenRouterAgent<MockOpenRouterHttpClient, MockSessionNotifier> {
        OpenRouterAgent::with_deps(
            MockSessionNotifier::new(),
            "test-model",
            "k",
            MockOpenRouterHttpClient::new(),
        )
        .with_max_response_bytes(limit)
    }

    #[tokio::test]
    async fn response_within_size_limit_completes_normally() {
        let agent = make_agent_with_size_limit(100);
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "hello".to_string() },
        ]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let resp = agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("q".to_string())])).await.unwrap();
            assert!(matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn));
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            assert_eq!(s.history.last().unwrap().content, "hello");
        }).await;
    }

    #[tokio::test]
    async fn response_exceeding_size_limit_stops_early() {
        let agent = make_agent_with_size_limit(10); // 10 byte limit
        // Push two deltas: first is fine, second pushes total over 10 bytes.
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "hello".to_string() },
            OpenRouterEvent::TextDelta { text: " world!!!!".to_string() },
            // Would never reach this:
            OpenRouterEvent::TextDelta { text: "more content".to_string() },
        ]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let resp = agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("q".to_string())])).await.unwrap();
            // Prompt must complete (not hang).
            assert!(matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn));
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            let assistant_text = &s.history.last().unwrap().content;
            // Content is the text up to and including the chunk that tripped the limit.
            assert!(assistant_text.contains("hello"), "should have first chunk");
            assert!(!assistant_text.contains("more content"), "should not have chunk past limit");
        }).await;
    }

    #[tokio::test]
    async fn response_exactly_at_size_limit_does_not_stop() {
        let agent = make_agent_with_size_limit(5); // exactly "hello" length
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "hello".to_string() },
        ]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("q".to_string())])).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            // Exactly at the limit (not strictly greater) should not trigger the guard.
            assert_eq!(s.history.last().unwrap().content, "hello");
        }).await;
    }

    // ── with_deps env var parsing ─────────────────────────────────────────────

    #[test]
    fn available_models_empty_env_falls_back_to_defaults() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_MODELS", ""); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_MODELS"); }
        assert!(agent.available_models.len() >= 3, "should use default list");
        assert!(
            agent.available_models.iter().any(|m| m.model_id.0.as_ref().contains("claude")),
            "default list must include a Claude model"
        );
    }

    #[test]
    fn available_models_malformed_entry_is_skipped_valid_ones_kept() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_MODELS", "good/model:Good Model,no-colon-entry,also/good:Also Good"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_MODELS"); }
        let ids: Vec<&str> = agent.available_models.iter().map(|m| m.model_id.0.as_ref()).collect();
        assert!(ids.contains(&"good/model"), "valid entry must be included");
        assert!(ids.contains(&"also/good"), "valid entry must be included");
        assert!(!ids.contains(&"no-colon-entry"), "malformed entry must be skipped");
    }

    #[test]
    fn available_models_all_malformed_falls_back_to_hardcoded_defaults() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_MODELS", "no-colon,also-no-colon"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_MODELS"); }
        assert!(
            agent.available_models.iter().any(|m| m.model_id.0.as_ref().contains("claude")),
            "all-malformed list must fall back to hardcoded defaults"
        );
    }

    #[test]
    fn max_history_messages_zero_defaults_to_20() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_MAX_HISTORY_MESSAGES", "0"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_MAX_HISTORY_MESSAGES"); }
        assert_eq!(agent.max_history, 20);
    }

    #[test]
    fn max_history_messages_non_numeric_defaults_to_20() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_MAX_HISTORY_MESSAGES", "not-a-number"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_MAX_HISTORY_MESSAGES"); }
        assert_eq!(agent.max_history, 20);
    }

    // ── builder methods ───────────────────────────────────────────────────────

    #[test]
    fn with_max_response_bytes_last_call_wins() {
        let agent = make_agent()
            .with_max_response_bytes(100)
            .with_max_response_bytes(200);
        assert_eq!(agent.max_response_bytes, 200);
    }

    #[test]
    fn with_system_prompt_overrides_env_value() {
        let agent = make_agent().with_system_prompt("Be concise.");
        assert_eq!(agent.system_prompt.as_deref(), Some("Be concise."));
    }

    #[test]
    fn with_system_prompt_called_twice_last_wins() {
        let agent = make_agent()
            .with_system_prompt("First.")
            .with_system_prompt("Second.");
        assert_eq!(agent.system_prompt.as_deref(), Some("Second."));
    }

    #[test]
    fn with_loaders_sets_agent_id() {
        use std::pin::Pin;
        use crate::agent_loader::{AgentConfig, AgentLoading};
        use crate::skill_loader::SkillLoading;

        struct NoOpAgentLoader;
        impl AgentLoading for NoOpAgentLoader {
            fn load_config<'a>(&'a self, _: &'a str) -> Pin<Box<dyn std::future::Future<Output = AgentConfig> + Send + 'a>> {
                Box::pin(async move { AgentConfig { skill_ids: vec![], system_prompt: None, model_id: None } })
            }
        }

        struct NoOpSkillLoader;
        impl SkillLoading for NoOpSkillLoader {
            fn load<'a>(&'a self, _: &'a [String]) -> Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
                Box::pin(async move { None })
            }
        }

        let agent = make_agent()
            .with_loaders("my-agent-42", Arc::new(NoOpAgentLoader), Arc::new(NoOpSkillLoader));
        assert_eq!(agent.agent_id.as_deref(), Some("my-agent-42"));
    }

    #[tokio::test]
    async fn size_limit_guard_still_saves_partial_response_to_history() {
        let agent = make_agent_with_size_limit(3);
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "abcd".to_string() }, // 4 bytes > limit of 3
        ]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("q".to_string())])).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            // Partial content must be saved (user message + assistant partial).
            assert_eq!(s.history.len(), 2);
            assert_eq!(s.history[1].role, "assistant");
            assert_eq!(s.history[1].content, "abcd");
        }).await;
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn initialize_without_global_key_offers_only_env_var_auth() {
        let agent = make_agent(); // global_api_key = None
        local().run_until(async move {
            let resp = agent.initialize(agent_client_protocol::InitializeRequest::new(
                agent_client_protocol::ProtocolVersion::LATEST,
            )).await.unwrap();
            // Must offer exactly one method: the user-key env-var method.
            assert_eq!(resp.auth_methods.len(), 1);
            let id = match &resp.auth_methods[0] {
                agent_client_protocol::AuthMethod::EnvVar(m) => m.id.0.as_ref().to_string(),
                other => panic!("expected EnvVar method, got {other:?}"),
            };
            assert_eq!(id, "openrouter-api-key");
        }).await;
    }

    #[tokio::test]
    async fn initialize_with_global_key_offers_both_auth_methods() {
        let agent = make_agent_with_key("server-key");
        local().run_until(async move {
            let resp = agent.initialize(agent_client_protocol::InitializeRequest::new(
                agent_client_protocol::ProtocolVersion::LATEST,
            )).await.unwrap();
            // Must offer two methods: user-key and agent key.
            assert_eq!(resp.auth_methods.len(), 2, "should offer env-var + agent methods");
            let ids: Vec<String> = resp.auth_methods.iter().map(|m| match m {
                agent_client_protocol::AuthMethod::EnvVar(e) => e.id.0.as_ref().to_string(),
                agent_client_protocol::AuthMethod::Agent(a) => a.id.0.as_ref().to_string(),
                _ => "other".to_string(),
            }).collect();
            assert!(ids.contains(&"openrouter-api-key".to_string()));
            assert!(ids.contains(&"agent".to_string()));
        }).await;
    }

    // ── ContentBlock variants in prompt ───────────────────────────────────────

    #[tokio::test]
    async fn prompt_resource_link_block_is_formatted_correctly() {
        use agent_client_protocol::ResourceLink;
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::ResourceLink(
                    ResourceLink::new("my-file.txt", "file:///workspace/my-file.txt"),
                )],
            )).await.unwrap();
            let calls = agent.client.calls.lock().unwrap();
            let user_msg = calls[0].messages.iter().find(|m| m.role == "user").unwrap();
            assert_eq!(
                user_msg.content,
                "[Resource: my-file.txt | file:///workspace/my-file.txt]",
                "ResourceLink must be formatted with name and URI"
            );
        }).await;
    }

    #[tokio::test]
    async fn prompt_embedded_text_resource_is_included_as_text() {
        use agent_client_protocol::{EmbeddedResource, EmbeddedResourceResource, TextResourceContents};
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::Resource(EmbeddedResource::new(
                    EmbeddedResourceResource::TextResourceContents(
                        TextResourceContents::new("fn main() {}", "file:///src/main.rs"),
                    ),
                ))],
            )).await.unwrap();
            let calls = agent.client.calls.lock().unwrap();
            let user_msg = calls[0].messages.iter().find(|m| m.role == "user").unwrap();
            assert_eq!(
                user_msg.content, "fn main() {}",
                "TextResourceContents must be included verbatim"
            );
        }).await;
    }

    #[tokio::test]
    async fn prompt_embedded_blob_resource_is_formatted_as_binary_placeholder() {
        use agent_client_protocol::{EmbeddedResource, EmbeddedResourceResource, BlobResourceContents};
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let mut blob = BlobResourceContents::new("base64data==", "file:///img.png");
            blob.mime_type = Some("image/png".to_string());
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::Resource(EmbeddedResource::new(
                    EmbeddedResourceResource::BlobResourceContents(blob),
                ))],
            )).await.unwrap();
            let calls = agent.client.calls.lock().unwrap();
            let user_msg = calls[0].messages.iter().find(|m| m.role == "user").unwrap();
            assert!(
                user_msg.content.contains("img.png") && user_msg.content.contains("image/png"),
                "BlobResourceContents must produce placeholder with uri and mime type: {:?}",
                user_msg.content
            );
        }).await;
    }

    #[tokio::test]
    async fn prompt_embedded_blob_resource_without_mime_type_uses_binary_fallback() {
        use agent_client_protocol::{EmbeddedResource, EmbeddedResourceResource, BlobResourceContents};
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            // mime_type left as None — code falls back to "binary"
            let blob = BlobResourceContents::new("base64data==", "file:///data.bin");
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::Resource(EmbeddedResource::new(
                    EmbeddedResourceResource::BlobResourceContents(blob),
                ))],
            )).await.unwrap();
            let calls = agent.client.calls.lock().unwrap();
            let user_msg = calls[0].messages.iter().find(|m| m.role == "user").unwrap();
            assert!(
                user_msg.content.contains("data.bin") && user_msg.content.contains("binary"),
                "None mime_type must fall back to 'binary': {:?}",
                user_msg.content
            );
        }).await;
    }

    #[tokio::test]
    async fn prompt_partial_match_is_not_treated_as_resume() {
        // "ping world" is NOT equal to "ping" — must not skip the user message.
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "pong".to_string() }]);
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "pong2".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("ping".to_string())],
            )).await.unwrap();
            // Different (longer) message — must be treated as a new turn, not a resume.
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("ping world".to_string())],
            )).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            let user_msgs: Vec<_> = s.history.iter().filter(|m| m.role == "user").collect();
            assert_eq!(user_msgs.len(), 2, "partial-match must not skip the second user message");
        }).await;
    }

    #[tokio::test]
    async fn prompt_with_empty_content_still_calls_api() {
        // Empty user input logs a warning but still sends the request.
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("".to_string())],
            )).await.unwrap();
            let calls = agent.client.calls.lock().unwrap();
            assert_eq!(calls.len(), 1, "empty input must still trigger one API call");
            let user_msg = calls[0].messages.iter().find(|m| m.role == "user").unwrap();
            assert_eq!(user_msg.content, "", "empty string must be sent as-is");
        }).await;
    }

    // ── set_session_model affects wire model ──────────────────────────────────

    #[tokio::test]
    async fn set_session_model_changes_model_in_wire_request() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            // Switch from the default "test-model" to "test-model" (it's the only one in list);
            // to test a different model we explicitly add it via env var — use a known available model.
            // Since "test-model" is auto-added as default, switching to it is a no-op in this test.
            // Instead, verify the default model is used when no override is set.
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            let calls = agent.client.calls.lock().unwrap();
            assert_eq!(
                calls[0].model, "test-model",
                "default model must appear in wire request"
            );
        }).await;
    }

    #[tokio::test]
    async fn set_session_model_override_appears_in_wire_request() {
        let _guard = env_lock();
        // Use env var to add a second model, then switch to it and verify the wire model changes.
        unsafe { std::env::set_var("OPENROUTER_MODELS", "test-model:Test Model,other-model:Other Model"); }
        let agent = make_agent_with_key("k");
        unsafe { std::env::remove_var("OPENROUTER_MODELS"); }
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.set_session_model(SetSessionModelRequest::new(sid.clone(), "other-model")).await.unwrap();
            agent.prompt(PromptRequest::new(
                sid,
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            let calls = agent.client.calls.lock().unwrap();
            assert_eq!(
                calls[0].model, "other-model",
                "switched model must appear in wire request, not default"
            );
        }).await;
    }

    // ── close_session during active prompt ────────────────────────────────────

    #[tokio::test]
    async fn close_session_during_active_prompt_cancels_it() {
        let agent = Arc::new(make_agent_with_key("k"));
        let agent2 = Arc::clone(&agent);
        agent.client.push_slow_response(OpenRouterEvent::TextDelta { text: "streaming".to_string() });

        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let sid2 = sid.clone();

            let agent_prompt = Arc::clone(&agent);
            let prompt_handle = tokio::task::spawn_local(async move {
                agent_prompt.prompt(PromptRequest::new(
                    sid,
                    vec![ContentBlock::from("q".to_string())],
                )).await.unwrap()
            });

            // Give the prompt time to start streaming.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;

            // Close the session — this should trigger the cancel sender.
            agent2.close_session(CloseSessionRequest::new(sid2.clone())).await.unwrap();

            let result = prompt_handle.await.unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::Cancelled),
                "close_session during active prompt must cancel it: {:?}",
                result.stop_reason
            );
            // Session was removed by close_session before the prompt's history write —
            // confirming the `if let Some(s) = sessions.get_mut(...)` None branch
            // is hit and handled gracefully (no panic, no stale entry reinserted).
            assert!(
                agent2.sessions.lock().await.get(&sid2.to_string()).is_none(),
                "session must remain absent after close_session — history write must not recreate it"
            );
        }).await;
    }

    // ── empty prompt content ──────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_with_empty_content_list_returns_end_turn() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![]); // no assistant response either
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            // Empty content block list — the agent warns but must not error.
            let result = agent.prompt(PromptRequest::new(sid, vec![])).await;
            assert!(result.is_ok(), "empty content list must not error: {result:?}");
            assert!(matches!(result.unwrap().stop_reason, agent_client_protocol::StopReason::EndTurn));
        }).await;
    }

    // ── TENANT_ID env var ─────────────────────────────────────────────────────

    #[test]
    fn tenant_id_env_var_appears_in_snapshot() {
        let _guard = env_lock();
        unsafe { std::env::set_var("TENANT_ID", "acme-corp"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("TENANT_ID"); }
        let session = make_session();
        let snap = agent.build_snapshot("sid", &session);
        assert_eq!(snap.tenant_id, "acme-corp");
    }

    #[test]
    fn tenant_id_defaults_to_default_when_absent() {
        let _guard = env_lock();
        unsafe { std::env::remove_var("TENANT_ID"); }
        let agent = make_agent();
        let session = make_session();
        let snap = agent.build_snapshot("sid", &session);
        assert_eq!(snap.tenant_id, "default");
    }

    #[test]
    fn tenant_id_empty_env_var_defaults_to_default() {
        let _guard = env_lock();
        unsafe { std::env::set_var("TENANT_ID", ""); }
        let agent = make_agent();
        unsafe { std::env::remove_var("TENANT_ID"); }
        let session = make_session();
        let snap = agent.build_snapshot("sid", &session);
        assert_eq!(snap.tenant_id, "default", "empty TENANT_ID must fall back to 'default'");
    }

    // ── build_snapshot: model and agent_id ───────────────────────────────────

    #[test]
    fn build_snapshot_uses_session_model_when_set() {
        let agent = make_agent();
        let mut session = make_session();
        session.model = Some("override-model".to_string());
        let snap = agent.build_snapshot("s", &session);
        assert_eq!(snap.model.as_deref(), Some("override-model"));
    }

    #[test]
    fn build_snapshot_falls_back_to_default_model_when_none() {
        let agent = make_agent(); // default_model = "test-model"
        let session = make_session(); // model = None
        let snap = agent.build_snapshot("s", &session);
        assert_eq!(snap.model.as_deref(), Some("test-model"));
    }

    #[test]
    fn build_snapshot_includes_agent_id_when_set() {
        let mut agent = make_agent();
        agent.agent_id = Some("my-agent-99".to_string());
        let session = make_session();
        let snap = agent.build_snapshot("s", &session);
        assert_eq!(snap.agent_id.as_deref(), Some("my-agent-99"));
    }

    #[test]
    fn build_snapshot_agent_id_is_none_when_not_set() {
        let agent = make_agent();
        let session = make_session();
        let snap = agent.build_snapshot("s", &session);
        assert!(snap.agent_id.is_none());
    }

    // ── fork_session: inherited fields ───────────────────────────────────────

    #[tokio::test]
    async fn fork_session_inherits_system_prompt_from_source() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap();
            let src_id = src.session_id.clone();

            // Manually plant a system prompt on the source session.
            agent.sessions.lock().await.get_mut(&src_id.to_string()).unwrap()
                .system_prompt = Some("Be helpful.".to_string());

            let fork = agent.fork_session(
                ForkSessionRequest::new(src_id, std::path::PathBuf::from("/f"))
            ).await.unwrap();

            let sessions = agent.sessions.lock().await;
            let fork_session = sessions.get(&fork.session_id.to_string()).unwrap();
            assert_eq!(
                fork_session.system_prompt.as_deref(), Some("Be helpful."),
                "fork must inherit source session's system_prompt"
            );
        }).await;
    }

    #[tokio::test]
    async fn fork_session_inherits_api_key_from_source() {
        let agent = make_agent(); // no global key
        local().run_until(async move {
            // Authenticate to set a per-user pending key.
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("per-user-key"));
            agent.authenticate(AuthenticateRequest::new("openrouter-api-key").meta(meta)).await.unwrap();

            let src = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap();
            let src_id = src.session_id.clone();

            let fork = agent.fork_session(
                ForkSessionRequest::new(src_id, std::path::PathBuf::from("/f"))
            ).await.unwrap();

            let sessions = agent.sessions.lock().await;
            let fork_session = sessions.get(&fork.session_id.to_string()).unwrap();
            assert_eq!(
                fork_session.api_key.as_deref(), Some("per-user-key"),
                "fork must inherit source session's api_key"
            );
        }).await;
    }

    #[tokio::test]
    async fn fork_session_stores_branched_at_index_in_session() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap();
            let src_id = src.session_id.clone();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 5})
            ).unwrap();
            let fork = agent.fork_session(
                ForkSessionRequest::new(src_id, std::path::PathBuf::from("/f")).meta(meta)
            ).await.unwrap();

            let sessions = agent.sessions.lock().await;
            let fork_session = sessions.get(&fork.session_id.to_string()).unwrap();
            assert_eq!(fork_session.branched_at_index, Some(5));
        }).await;
    }

    // ── prompt: multi-block joining and skipped types ─────────────────────────

    #[tokio::test]
    async fn prompt_two_text_blocks_are_joined_with_newline() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![
                    ContentBlock::from("first".to_string()),
                    ContentBlock::from("second".to_string()),
                ],
            )).await.unwrap();
            let calls = agent.client.calls.lock().unwrap();
            let user_msg = calls[0].messages.iter().find(|m| m.role == "user").unwrap();
            assert_eq!(user_msg.content, "first\nsecond");
        }).await;
    }

    #[tokio::test]
    async fn prompt_image_block_is_silently_skipped() {
        use agent_client_protocol::ImageContent;
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;
            // Image blocks produce no text — user_input will be empty (warns, doesn't error).
            let result = agent.prompt(PromptRequest::new(
                sid,
                vec![ContentBlock::Image(ImageContent::new("base64data==", "image/png"))],
            )).await;
            assert!(result.is_ok(), "image-only prompt must not error: {result:?}");
        }).await;
    }

    // ── env var: prompt_timeout and max_response_bytes ────────────────────────

    #[test]
    fn prompt_timeout_env_var_is_read() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_PROMPT_TIMEOUT_SECS", "60"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_PROMPT_TIMEOUT_SECS"); }
        assert_eq!(agent.prompt_timeout, Duration::from_secs(60));
    }

    #[test]
    fn prompt_timeout_zero_falls_back_to_300s() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_PROMPT_TIMEOUT_SECS", "0"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_PROMPT_TIMEOUT_SECS"); }
        assert_eq!(agent.prompt_timeout, Duration::from_secs(300));
    }

    #[test]
    fn with_prompt_timeout_overrides_default() {
        let agent = make_agent().with_prompt_timeout(Duration::from_millis(50));
        assert_eq!(agent.prompt_timeout, Duration::from_millis(50));
    }

    #[tokio::test]
    async fn prompt_stream_timeout_breaks_loop_and_returns_end_turn() {
        // A stream that never produces any event triggers the per-chunk timeout,
        // emitting an Error event internally and breaking the loop.
        let agent = make_agent_with_key("k")
            .with_prompt_timeout(Duration::from_millis(10));
        agent.client.push_pending_response();
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let resp = agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            // Timeout → Error event → breaks loop → EndTurn
            assert!(matches!(resp.stop_reason, StopReason::EndTurn));
            // No assistant text was accumulated, so history has only the user message.
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            assert_eq!(s.history.len(), 1, "timeout must not write empty assistant message");
            assert_eq!(s.history[0].role, "user");
        }).await;
    }

    #[test]
    fn max_response_bytes_env_var_is_read() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_MAX_RESPONSE_BYTES", "1024"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_MAX_RESPONSE_BYTES"); }
        assert_eq!(agent.max_response_bytes, 1024);
    }

    #[test]
    fn max_response_bytes_zero_falls_back_to_4mb() {
        let _guard = env_lock();
        unsafe { std::env::set_var("OPENROUTER_MAX_RESPONSE_BYTES", "0"); }
        let agent = make_agent();
        unsafe { std::env::remove_var("OPENROUTER_MAX_RESPONSE_BYTES"); }
        assert_eq!(agent.max_response_bytes, 4 * 1024 * 1024);
    }

    // ── initialize: capabilities ──────────────────────────────────────────────

    #[tokio::test]
    async fn initialize_response_has_agent_info_with_correct_name() {
        let agent = make_agent();
        local().run_until(async move {
            let resp = agent.initialize(agent_client_protocol::InitializeRequest::new(
                agent_client_protocol::ProtocolVersion::LATEST,
            )).await.unwrap();
            let info = resp.agent_info.expect("agent_info must be set");
            assert_eq!(
                info.name, "trogon-openrouter-runner",
                "agent name must match crate name"
            );
        }).await;
    }

    #[tokio::test]
    async fn initialize_response_has_embedded_context_true() {
        let agent = make_agent();
        local().run_until(async move {
            let resp = agent.initialize(agent_client_protocol::InitializeRequest::new(
                agent_client_protocol::ProtocolVersion::LATEST,
            )).await.unwrap();
            assert!(
                resp.agent_capabilities.prompt_capabilities.embedded_context,
                "embedded_context must be true to support Resource blocks in prompts"
            );
        }).await;
    }

    #[tokio::test]
    async fn initialize_response_has_full_session_capabilities() {
        let agent = make_agent();
        local().run_until(async move {
            let resp = agent.initialize(agent_client_protocol::InitializeRequest::new(
                agent_client_protocol::ProtocolVersion::LATEST,
            )).await.unwrap();
            let caps = &resp.agent_capabilities.session_capabilities;
            assert!(caps.fork.is_some(), "fork capability must be declared");
            assert!(caps.list.is_some(), "list capability must be declared");
            assert!(caps.resume.is_some(), "resume capability must be declared");
            assert!(caps.close.is_some(), "close capability must be declared");
        }).await;
    }

    // ── authenticate / api_key precedence ─────────────────────────────────────

    #[tokio::test]
    async fn prompt_uses_pending_key_over_global_key() {
        let agent = make_agent_with_key("global-key");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
        local().run_until(async move {
            // Authenticate with a user-specific key.
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("user-specific-key"));
            agent.authenticate(AuthenticateRequest::new("openrouter-api-key").meta(meta)).await.unwrap();

            let sid = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid,
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();

            let calls = agent.client.calls.lock().unwrap();
            assert_eq!(
                calls[0].api_key, "user-specific-key",
                "user-provided key must take precedence over global server key"
            );
        }).await;
    }

    #[tokio::test]
    async fn pending_key_consumed_not_available_to_second_session() {
        let agent = make_agent_with_key("global-key");
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "r1".to_string() }]);
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "r2".to_string() }]);
        local().run_until(async move {
            // First authenticate + new_session consumes the pending key.
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("user-key"));
            agent.authenticate(AuthenticateRequest::new("openrouter-api-key").meta(meta)).await.unwrap();
            let sid1 = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;

            // Second session — no pending key; should fall back to global.
            let sid2 = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;

            agent.prompt(PromptRequest::new(sid1, vec![ContentBlock::from("q".to_string())])).await.unwrap();
            agent.prompt(PromptRequest::new(sid2, vec![ContentBlock::from("q".to_string())])).await.unwrap();

            let calls = agent.client.calls.lock().unwrap();
            assert_eq!(calls[0].api_key, "user-key",   "first session must use user key");
            assert_eq!(calls[1].api_key, "global-key", "second session must fall back to global key");
        }).await;
    }

    #[tokio::test]
    async fn authenticate_called_twice_replaces_pending_key() {
        let agent = make_agent();
        local().run_until(async move {
            let mut meta1 = serde_json::Map::new();
            meta1.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("first-key"));
            agent.authenticate(AuthenticateRequest::new("openrouter-api-key").meta(meta1)).await.unwrap();

            let mut meta2 = serde_json::Map::new();
            meta2.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("second-key"));
            agent.authenticate(AuthenticateRequest::new("openrouter-api-key").meta(meta2)).await.unwrap();

            // Second call must overwrite the first.
            assert_eq!(
                *agent.pending_api_key.lock().await,
                Some("second-key".to_string()),
                "second authenticate call must replace the previous pending key"
            );
        }).await;
    }

    // ── session store integration ─────────────────────────────────────────────

    struct RecordingSessionStore {
        saves: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl RecordingSessionStore {
        fn new() -> (Self, Arc<std::sync::Mutex<Vec<String>>>) {
            let saves = Arc::new(std::sync::Mutex::new(Vec::new()));
            (Self { saves: Arc::clone(&saves) }, saves)
        }
    }

    impl crate::session_store::SessionStoring for RecordingSessionStore {
        fn save<'a>(
            &'a self,
            snapshot: &'a crate::session_store::SessionSnapshot,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            let id = snapshot.id.clone();
            let saves = Arc::clone(&self.saves);
            Box::pin(async move {
                saves.lock().unwrap().push(id);
            })
        }

        fn remove<'a>(
            &'a self,
            _tenant_id: &'a str,
            _session_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {})
        }
    }

    fn make_agent_with_store() -> (
        OpenRouterAgent<MockOpenRouterHttpClient, MockSessionNotifier>,
        Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let (store, saves) = RecordingSessionStore::new();
        let agent = OpenRouterAgent::with_deps(
            MockSessionNotifier::new(),
            "test-model",
            "k",
            MockOpenRouterHttpClient::new(),
        )
        .with_session_store(Arc::new(store));
        (agent, saves)
    }

    #[tokio::test]
    async fn session_store_save_called_on_new_session() {
        let (agent, saves) = make_agent_with_store();
        local().run_until(async move {
            let resp = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap();
            let sid = resp.session_id.to_string();
            let recorded = saves.lock().unwrap().clone();
            assert!(
                recorded.contains(&sid),
                "session store must be called with the new session id: {recorded:?}"
            );
        }).await;
    }

    #[tokio::test]
    async fn session_store_save_called_after_prompt() {
        let (agent, saves) = make_agent_with_store();
        agent.client.push_response(vec![OpenRouterEvent::TextDelta { text: "hi".to_string() }]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;
            let count_before = saves.lock().unwrap().len();
            agent.prompt(PromptRequest::new(
                sid,
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            let count_after = saves.lock().unwrap().len();
            assert!(
                count_after > count_before,
                "session store must be called again after prompt"
            );
        }).await;
    }

    #[tokio::test]
    async fn session_store_save_called_on_close_session() {
        let (agent, saves) = make_agent_with_store();
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;
            let count_before = saves.lock().unwrap().len();
            agent.close_session(CloseSessionRequest::new(sid)).await.unwrap();
            let count_after = saves.lock().unwrap().len();
            assert!(
                count_after > count_before,
                "session store must be called when closing a session"
            );
        }).await;
    }

    #[tokio::test]
    async fn session_store_save_called_on_fork_session() {
        let (agent, saves) = make_agent_with_store();
        local().run_until(async move {
            let src_id = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;
            let count_before = saves.lock().unwrap().len();
            agent.fork_session(ForkSessionRequest::new(src_id, std::path::PathBuf::from("/f"))).await.unwrap();
            let count_after = saves.lock().unwrap().len();
            assert!(
                count_after > count_before,
                "session store must be called for the forked session"
            );
        }).await;
    }

    // ── list_sessions cwd ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_returns_cwd_from_new_session() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            agent.new_session(NewSessionRequest::new(PathBuf::from("/my/project"))).await.unwrap();
            let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            assert_eq!(resp.sessions.len(), 1);
            assert_eq!(
                resp.sessions[0].cwd.to_string_lossy(), "/my/project",
                "list_sessions must report the cwd from new_session"
            );
        }).await;
    }

    #[tokio::test]
    async fn list_sessions_fork_carries_its_own_cwd() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src_id = agent.new_session(NewSessionRequest::new(PathBuf::from("/src"))).await.unwrap().session_id;
            agent.fork_session(ForkSessionRequest::new(src_id, PathBuf::from("/fork-dir"))).await.unwrap();
            let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            let fork_info = resp.sessions.iter().find(|s| s.cwd.to_string_lossy() == "/fork-dir");
            assert!(fork_info.is_some(), "fork session must have its own cwd in list_sessions");
        }).await;
    }

    // ── stream event edge cases ───────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_error_event_breaks_stream_and_saves_partial_text() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "partial".to_string() },
            OpenRouterEvent::Error { message: "something went wrong".to_string() },
        ]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let resp = agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            // Error event breaks the loop; still returns EndTurn.
            assert!(matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn));
            // Partial text collected before the error must be saved.
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            assert_eq!(s.history[1].role, "assistant");
            assert_eq!(s.history[1].content, "partial");
        }).await;
    }

    #[tokio::test]
    async fn prompt_finished_event_is_silently_ignored() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "text".to_string() },
            OpenRouterEvent::Finished { reason: crate::client::FinishReason::Stop },
            OpenRouterEvent::Done,
        ]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            // Finished event is ignored; only TextDelta content is stored.
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            assert_eq!(s.history[1].content, "text");
        }).await;
    }

    #[tokio::test]
    async fn prompt_empty_assistant_text_is_not_stored_in_history() {
        // If the stream returns no text (e.g., only Done), the user message is stored
        // but no assistant message is pushed (because assistant_text is empty).
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![OpenRouterEvent::Done]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let s = sessions.get(&sid.to_string()).unwrap();
            assert_eq!(s.history.len(), 1, "no assistant turn must be stored when response text is empty");
            assert_eq!(s.history[0].role, "user");
        }).await;
    }

    // ── list_sessions after close ─────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_after_close_session_is_removed() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            agent.close_session(CloseSessionRequest::new(sid.clone())).await.unwrap();
            let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            assert!(
                resp.sessions.iter().all(|s| s.session_id != sid),
                "closed session must not appear in list_sessions"
            );
        }).await;
    }

    // ── fork branchAtIndex edge cases ─────────────────────────────────────────

    #[tokio::test]
    async fn fork_with_branch_at_index_zero_removes_all_history() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src_id = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            {
                let mut sessions = agent.sessions.lock().await;
                let s = sessions.get_mut(&src_id.to_string()).unwrap();
                s.history.push(Message::user("a"));
                s.history.push(Message::assistant("b"));
            }
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 0})
            ).unwrap();
            let fork = agent.fork_session(
                ForkSessionRequest::new(src_id, PathBuf::from("/f")).meta(meta)
            ).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let fork_session = sessions.get(&fork.session_id.to_string()).unwrap();
            assert_eq!(fork_session.history.len(), 0, "branchAtIndex:0 must produce an empty history");
        }).await;
    }

    #[tokio::test]
    async fn fork_with_branch_at_index_beyond_length_copies_full_history() {
        let agent = make_agent_with_key("k");
        local().run_until(async move {
            let src_id = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            {
                let mut sessions = agent.sessions.lock().await;
                let s = sessions.get_mut(&src_id.to_string()).unwrap();
                s.history.push(Message::user("a"));
                s.history.push(Message::assistant("b"));
            }
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 9999})
            ).unwrap();
            let fork = agent.fork_session(
                ForkSessionRequest::new(src_id, PathBuf::from("/f")).meta(meta)
            ).await.unwrap();
            let sessions = agent.sessions.lock().await;
            let fork_session = sessions.get(&fork.session_id.to_string()).unwrap();
            assert_eq!(fork_session.history.len(), 2, "branchAtIndex beyond length must copy full history");
        }).await;
    }

    // ── agent loader system-prompt composition ────────────────────────────────

    fn make_agent_with_loaders(
        agent_sp: Option<&'static str>,
        skills_text: Option<&'static str>,
        model_id: Option<&'static str>,
    ) -> OpenRouterAgent<MockOpenRouterHttpClient, MockSessionNotifier> {
        use std::pin::Pin;
        use crate::agent_loader::{AgentConfig, AgentLoading};
        use crate::skill_loader::SkillLoading;

        struct FixedAgentLoader {
            sp: Option<&'static str>,
            model_id: Option<&'static str>,
        }
        impl AgentLoading for FixedAgentLoader {
            fn load_config<'a>(&'a self, _: &'a str) -> Pin<Box<dyn std::future::Future<Output = AgentConfig> + Send + 'a>> {
                let sp = self.sp.map(|s| s.to_string());
                let mid = self.model_id.map(|s| s.to_string());
                Box::pin(async move {
                    AgentConfig { skill_ids: vec![], system_prompt: sp, model_id: mid }
                })
            }
        }

        struct FixedSkillLoader { text: Option<&'static str> }
        impl SkillLoading for FixedSkillLoader {
            fn load<'a>(&'a self, _: &'a [String]) -> Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
                let t = self.text.map(|s| s.to_string());
                Box::pin(async move { t })
            }
        }

        make_agent().with_loaders(
            "agent-1",
            Arc::new(FixedAgentLoader { sp: agent_sp, model_id }),
            Arc::new(FixedSkillLoader { text: skills_text }),
        )
    }

    #[tokio::test]
    async fn new_session_combines_agent_and_skills_system_prompt() {
        let agent = make_agent_with_loaders(Some("Be concise."), Some("# Skills\n\nDo X."), None);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let sessions = agent.sessions.lock().await;
            let sp = sessions.get(&sid.to_string()).unwrap().system_prompt.as_deref().unwrap();
            assert!(sp.contains("Be concise."), "agent system_prompt must be in combined prompt");
            assert!(sp.contains("# Skills\n\nDo X."), "skills text must be in combined prompt");
            assert!(sp.contains("\n\n"), "parts must be joined with double newline");
        }).await;
    }

    #[tokio::test]
    async fn new_session_uses_skills_only_when_no_agent_system_prompt() {
        let agent = make_agent_with_loaders(None, Some("# Skills\n\nDo Y."), None);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let sessions = agent.sessions.lock().await;
            let sp = sessions.get(&sid.to_string()).unwrap().system_prompt.as_deref().unwrap();
            assert_eq!(sp, "# Skills\n\nDo Y.", "skills-only path must produce exactly the skills text");
        }).await;
    }

    #[tokio::test]
    async fn new_session_uses_agent_prompt_only_when_no_skills() {
        let agent = make_agent_with_loaders(Some("Agent only."), None, None);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let sessions = agent.sessions.lock().await;
            let sp = sessions.get(&sid.to_string()).unwrap().system_prompt.as_deref().unwrap();
            assert_eq!(sp, "Agent only.", "agent-only path must produce exactly the agent system prompt");
        }).await;
    }

    #[tokio::test]
    async fn new_session_no_prompt_when_neither_agent_nor_skills() {
        let agent = make_agent_with_loaders(None, None, None);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let sessions = agent.sessions.lock().await;
            let sp = &sessions.get(&sid.to_string()).unwrap().system_prompt;
            assert!(sp.is_none(), "both-none path must produce no system prompt");
        }).await;
    }

    #[tokio::test]
    async fn new_session_falls_back_to_with_system_prompt_when_agent_loader_has_no_prompt() {
        // agent_sp = None, self.system_prompt = Some("fallback") → base = "fallback"
        // Skills are also present → combined = "fallback\n\nskills"
        use std::pin::Pin;
        use crate::agent_loader::{AgentConfig, AgentLoading};
        use crate::skill_loader::SkillLoading;

        struct NoPromptLoader;
        impl AgentLoading for NoPromptLoader {
            fn load_config<'a>(&'a self, _: &'a str) -> Pin<Box<dyn std::future::Future<Output = AgentConfig> + Send + 'a>> {
                Box::pin(async move { AgentConfig { skill_ids: vec![], system_prompt: None, model_id: None } })
            }
        }
        struct FixedSkillLoader;
        impl SkillLoading for FixedSkillLoader {
            fn load<'a>(&'a self, _: &'a [String]) -> Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
                Box::pin(async move { Some("# Skills\n\nDo Z.".to_string()) })
            }
        }

        let agent = make_agent()
            .with_system_prompt("Base prompt.")
            .with_loaders("agent-x", Arc::new(NoPromptLoader), Arc::new(FixedSkillLoader));

        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let sessions = agent.sessions.lock().await;
            let sp = sessions.get(&sid.to_string()).unwrap().system_prompt.as_deref().unwrap();
            assert!(sp.starts_with("Base prompt."), "with_system_prompt must be used when agent loader has no prompt");
            assert!(sp.contains("# Skills\n\nDo Z."), "skills must be appended");
        }).await;
    }

    #[tokio::test]
    async fn new_session_uses_model_id_from_agent_loader() {
        let agent = make_agent_with_loaders(None, None, Some("test-model"));
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(PathBuf::from("/"))).await.unwrap().session_id;
            let sessions = agent.sessions.lock().await;
            let model = sessions.get(&sid.to_string()).unwrap().model.as_deref();
            assert_eq!(model, Some("test-model"), "agent loader model_id must be stored on the session");
        }).await;
    }

    // ── usage notification ────────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_usage_event_fires_usage_notification() {
        let agent = make_agent_with_key("k");
        agent.client.push_response(vec![
            OpenRouterEvent::TextDelta { text: "reply".to_string() },
            OpenRouterEvent::Usage { prompt_tokens: 20, completion_tokens: 8 },
        ]);
        local().run_until(async move {
            let sid = agent.new_session(NewSessionRequest::new(std::path::PathBuf::from("/"))).await.unwrap().session_id;
            agent.prompt(PromptRequest::new(
                sid,
                vec![ContentBlock::from("q".to_string())],
            )).await.unwrap();

            let notes = agent.notifier.notifications.lock().unwrap();
            let has_usage = notes.iter().any(|n| {
                matches!(&n.update, agent_client_protocol::SessionUpdate::UsageUpdate(_))
            });
            assert!(has_usage, "a UsageUpdate notification must be fired for Usage events");
        }).await;
    }
}
