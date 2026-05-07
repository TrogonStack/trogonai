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
