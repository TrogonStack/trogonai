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
}
