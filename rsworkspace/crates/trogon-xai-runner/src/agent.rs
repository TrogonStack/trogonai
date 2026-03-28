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
    StopReason,
};
use agent_client_protocol::Client as _;
use async_trait::async_trait;
use futures_util::StreamExt as _;
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::client::{Message, XaiClient, XaiEvent};

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

struct XaiSession {
    cwd: String,
    model: Option<String>,
    history: Vec<Message>,
    /// Set by `cancel` to interrupt an in-flight `prompt`.
    cancel: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

/// ACP Agent implementation backed by xAI's Grok API (OpenAI-compatible REST).
///
/// Each `XaiAgent` manages multiple sessions, each holding its own conversation
/// history. Prompt calls stream chat completions from `api.x.ai` and forward
/// text chunks to the ACP client via NATS `SessionNotification`s.
///
/// Authentication uses the `XAI_API_KEY` environment variable.
pub struct XaiAgent {
    nats: async_nats::Client,
    acp_prefix: AcpPrefix,
    client: Arc<XaiClient>,
    sessions: Arc<Mutex<HashMap<String, XaiSession>>>,
    default_model: String,
    prompt_timeout: Duration,
    available_models: Vec<ModelInfo>,
}

impl XaiAgent {
    /// Create a new `XaiAgent`.
    ///
    /// Environment variables:
    /// - `XAI_API_KEY` — xAI API key (required at construction)
    /// - `XAI_PROMPT_TIMEOUT_SECS` — prompt timeout in seconds (default: 300)
    /// - `XAI_MODELS` — comma-separated `id:label` pairs
    ///   (default: `grok-3:Grok 3,grok-3-mini:Grok 3 Mini`)
    pub fn new(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let default_model: String = default_model.into();

        let prompt_timeout = std::env::var("XAI_PROMPT_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
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

        Self {
            nats,
            acp_prefix,
            client: Arc::new(XaiClient::new(api_key)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            default_model,
            prompt_timeout,
            available_models,
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
}

#[async_trait(?Send)]
impl agent_client_protocol::Agent for XaiAgent {
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
        _req: AuthenticateRequest,
    ) -> agent_client_protocol::Result<AuthenticateResponse> {
        // Auth is handled via XAI_API_KEY at construction — no per-session auth.
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(
        &self,
        req: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        let cwd = req.cwd.to_string_lossy().into_owned();
        let session_id = Uuid::new_v4().to_string();

        self.sessions.lock().await.insert(
            session_id.clone(),
            XaiSession {
                cwd,
                model: None,
                history: Vec::new(),
                cancel: Arc::new(Mutex::new(None)),
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
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
        let model = session.model.as_deref();
        Ok(LoadSessionResponse::new()
            .modes(self.session_mode_state())
            .models(self.session_model_state(model)))
    }

    async fn resume_session(
        &self,
        req: ResumeSessionRequest,
    ) -> agent_client_protocol::Result<ResumeSessionResponse> {
        let session_id = req.session_id.to_string();
        // Conversation history is in memory — just verify the session exists.
        self.sessions
            .lock()
            .await
            .get(&session_id)
            .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
        Ok(ResumeSessionResponse::new())
    }

    async fn fork_session(
        &self,
        req: ForkSessionRequest,
    ) -> agent_client_protocol::Result<ForkSessionResponse> {
        let source_id = req.session_id.to_string();
        let cwd = req.cwd.to_string_lossy().into_owned();

        let (inherited_model, cloned_history) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&source_id)
                .ok_or_else(|| internal_error(format!("session {source_id} not found")))?;
            (s.model.clone(), s.history.clone())
        };

        let new_session_id = Uuid::new_v4().to_string();
        self.sessions.lock().await.insert(
            new_session_id.clone(),
            XaiSession {
                cwd,
                model: inherited_model.clone(),
                history: cloned_history,
                cancel: Arc::new(Mutex::new(None)),
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
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        if user_input.is_empty() {
            warn!(session_id, "xai: prompt contains no text blocks");
        }

        // Pull session state and install a cancel channel — release the lock
        // before streaming so cancel() can acquire it.
        let (model, history, cancel_arc) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(&session_id)
                .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
            (s.model.clone(), s.history.clone(), Arc::clone(&s.cancel))
        };

        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        cancel_arc.lock().await.replace(cancel_tx);

        let model = model.as_deref().unwrap_or(&self.default_model).to_string();

        // Build messages: history + new user turn.
        let mut messages = history.clone();
        messages.push(Message { role: "user".to_string(), content: user_input.clone() });

        let client = Arc::clone(&self.client);
        let mut stream = client.chat_stream(&model, &messages).await;

        let mut assistant_text = String::new();
        let mut canceled = false;

        let stop_reason = 'turn: loop {
            let event = tokio::select! {
                biased;
                _ = &mut cancel_rx => {
                    info!(session_id, "xai: prompt canceled");
                    canceled = true;
                    break 'turn StopReason::EndTurn;
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
                XaiEvent::Done => break 'turn StopReason::EndTurn,
                XaiEvent::Error { message } => {
                    warn!(session_id, error = %message, "xai: stream error");
                    break 'turn StopReason::EndTurn;
                }
            }
        };

        // Append exchange to history only on successful (non-canceled) turns.
        if !canceled && !assistant_text.is_empty() {
            let mut sessions = self.sessions.lock().await;
            if let Some(session) = sessions.get_mut(&session_id) {
                session.history.push(Message {
                    role: "user".to_string(),
                    content: user_input,
                });
                session.history.push(Message {
                    role: "assistant".to_string(),
                    content: assistant_text,
                });
            }
        }

        Ok(PromptResponse::new(stop_reason))
    }

    async fn cancel(
        &self,
        req: CancelNotification,
    ) -> agent_client_protocol::Result<()> {
        let session_id = req.session_id.to_string();
        let cancel_arc = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_id).map(|s| Arc::clone(&s.cancel))
        };
        if let Some(cancel_arc) = cancel_arc {
            if let Some(tx) = cancel_arc.lock().await.take() {
                let _ = tx.send(());
            }
        }
        Ok(())
    }
}

#[cfg(feature = "test-helpers")]
impl XaiAgent {
    pub async fn test_session_history(&self, id: &str) -> Vec<crate::client::Message> {
        self.sessions.lock().await.get(id).map(|s| s.history.clone()).unwrap_or_default()
    }

    pub async fn test_session_model(&self, id: &str) -> Option<String> {
        self.sessions.lock().await.get(id).and_then(|s| s.model.clone())
    }
}
