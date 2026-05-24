use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{
    AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    CloseSessionRequest, CloseSessionResponse, ContentBlock, ContentChunk, Error, ErrorCode,
    ExtRequest, ExtResponse, ForkSessionRequest, ForkSessionResponse, Implementation,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, LoadSessionResponse, ModelInfo, NewSessionRequest, NewSessionResponse,
    PromptRequest, PromptResponse, ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse,
    SessionCapabilities, SessionCloseCapabilities, SessionForkCapabilities, SessionId, SessionInfo,
    SessionListCapabilities, SessionMode, SessionModeState, SessionModelState, SessionNotification,
    SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
    SetSessionModelRequest, SetSessionModelResponse, StopReason, ToolCall, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use crate::process::{CodexEvent, RealProcessSpawner};
use crate::traits::{CodexProcessClient, ProcessSpawner, SessionNotifier, SessionNotifierFactory};

// ── ProcessGuard ──────────────────────────────────────────────────────────────

/// Holds the process mutex lock and derefs directly to `P`,
/// encoding the post-condition of `process()` (always `Some`) in the type
/// instead of requiring callers to call `.as_ref().unwrap()`.
struct ProcessGuard<P>(tokio::sync::OwnedMutexGuard<Option<P>>);

impl<P> std::ops::Deref for ProcessGuard<P> {
    type Target = P;
    fn deref(&self) -> &P {
        self.0
            .as_ref()
            .expect("CodexProcess guaranteed present by process()")
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

fn invalid_params(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidParams.into(), msg.into())
}

// ── Session ───────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct CodexSession {
    thread_id: String,
    cwd: String,
    /// Per-session model override. None means use the agent default.
    model: Option<String>,
    /// Session this was branched from. None for root sessions.
    parent_session_id: Option<String>,
    history: Vec<trogon_runner_tools::portable_session::PortableMessage>,
    pending_history: Option<Vec<trogon_runner_tools::portable_session::PortableMessage>>,
    first_turn: bool,
}

// ── NatsNotifierFactory ───────────────────────────────────────────────────────

/// Production [`SessionNotifierFactory`] backed by a real NATS connection.
pub struct NatsNotifierFactory {
    nats: async_nats::Client,
    acp_prefix: AcpPrefix,
}

impl NatsNotifierFactory {
    pub fn new(nats: async_nats::Client, acp_prefix: AcpPrefix) -> Self {
        Self { nats, acp_prefix }
    }
}

impl SessionNotifierFactory for NatsNotifierFactory {
    type Notifier = NatsClientProxy<async_nats::Client>;

    fn make_notifier(
        &self,
        session_id: &SessionId,
    ) -> agent_client_protocol::Result<Self::Notifier> {
        let acp_session_id =
            AcpSessionId::try_from(session_id).map_err(|e| internal_error(e.to_string()))?;
        Ok(NatsClientProxy::new(
            self.nats.clone(),
            acp_session_id,
            self.acp_prefix.clone(),
            Duration::from_secs(30),
        ))
    }
}

#[async_trait(?Send)]
impl SessionNotifier for NatsClientProxy<async_nats::Client> {
    async fn session_notification(
        &self,
        notif: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        agent_client_protocol::Client::session_notification(self, notif).await
    }
}

// ── CodexAgent ────────────────────────────────────────────────────────────────

/// ACP Agent implementation backed by a `codex app-server` subprocess.
///
/// Each `CodexAgent` manages one `codex app-server` process. ACP sessions map
/// to Codex threads (thread_id). Prompt calls run Codex turns and stream the
/// resulting events back to the ACP client as `SessionNotification`s.
///
/// The subprocess is spawned lazily and re-spawned automatically if it crashes.
/// Re-spawning clears all in-memory sessions since Codex thread state is lost.
///
/// Generic parameters:
/// - `N`: factory that creates session notifiers (production: [`NatsNotifierFactory`])
/// - `P`: spawner for the codex subprocess (production: [`RealProcessSpawner`])
pub struct CodexAgent<N: SessionNotifierFactory, P: ProcessSpawner> {
    notifier_factory: N,
    spawner: P,
    process: Arc<Mutex<Option<P::Process>>>,
    sessions: Arc<Mutex<HashMap<String, CodexSession>>>,
    default_model: String,
    prompt_timeout: Duration,
    available_models: Vec<ModelInfo>,
}

/// Convenience type alias for the production agent wired to real dependencies.
pub type DefaultCodexAgent = CodexAgent<NatsNotifierFactory, RealProcessSpawner>;

impl<N, P> CodexAgent<N, P>
where
    N: SessionNotifierFactory,
    P: ProcessSpawner + 'static,
    P::Process: 'static,
{
    /// Create a new `CodexAgent`. The subprocess is spawned lazily on the first
    /// call that needs it.
    ///
    /// Environment variables read once at construction:
    /// - `CODEX_PROMPT_TIMEOUT_SECS` — prompt timeout in seconds (default: 7200)
    /// - `CODEX_MODELS` — comma-separated `id:label` pairs (default: `o4-mini:o4-mini,o3:o3,gpt-4o:GPT-4o`)
    pub fn new(notifier_factory: N, spawner: P, default_model: impl Into<String>) -> Self {
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
                    .filter_map(|entry| match entry.split_once(':') {
                        Some((id, label)) => Some(ModelInfo::new(
                            id.trim().to_string(),
                            label.trim().to_string(),
                        )),
                        None => {
                            warn!(
                                entry,
                                "CODEX_MODELS: skipping malformed entry (expected 'id:label')"
                            );
                            None
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
        if !available_models
            .iter()
            .any(|m| m.model_id.0.as_ref() == default_model.as_str())
        {
            warn!(model = %default_model, "default model not in available list; adding it");
            available_models.push(ModelInfo::new(default_model.clone(), default_model.clone()));
        }

        Self {
            notifier_factory,
            spawner,
            process: Arc::new(Mutex::new(None)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            default_model,
            prompt_timeout,
            available_models,
        }
    }

    /// Ensures the process is running, spawning (or re-spawning) as needed,
    /// and returns a guard that holds the mutex and derefs to `&P::Process`.
    ///
    /// If the previous process has exited, in-memory sessions are cleared —
    /// Codex thread state is stored in the subprocess, so they cannot be recovered.
    async fn process(&self) -> Result<ProcessGuard<P::Process>, Error> {
        let mut guard = Arc::clone(&self.process).lock_owned().await;
        let needs_spawn = guard.as_ref().is_none_or(|p| !p.is_alive());
        if needs_spawn {
            if guard.is_some() {
                warn!("codex app-server exited; re-spawning (existing sessions invalidated)");
                self.sessions.lock().await.clear();
            }
            match self.spawner.spawn().await {
                Ok(p) => *guard = Some(p),
                Err(e) => {
                    return Err(internal_error(format!(
                        "failed to spawn codex app-server: {e}"
                    )));
                }
            }
        }
        Ok(ProcessGuard(guard))
    }

    fn make_notifier(&self, session_id: &SessionId) -> agent_client_protocol::Result<N::Notifier> {
        self.notifier_factory.make_notifier(session_id)
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

impl DefaultCodexAgent {
    /// Convenience constructor that wires the production NATS notifier and
    /// real process spawner. This is what [`main`] calls.
    pub fn with_nats(
        nats: async_nats::Client,
        acp_prefix: AcpPrefix,
        default_model: impl Into<String>,
    ) -> Self {
        Self::new(
            NatsNotifierFactory::new(nats, acp_prefix),
            RealProcessSpawner,
            default_model,
        )
    }
}

// ── ACP Agent impl ────────────────────────────────────────────────────────────

// Note: `branchAtIndex` is intentionally not supported. Codex manages its own
// conversation history inside the subprocess via `thread_fork` — the ACP layer
// has no access to individual messages, so truncation at an arbitrary index is
// not possible. `session/list_children` IS supported via in-memory HashMap scan
// (same approach as xai-runner). Results are ephemeral: a process restart clears
// all sessions.
#[async_trait(?Send)]
impl<N, P> agent_client_protocol::Agent for CodexAgent<N, P>
where
    N: SessionNotifierFactory,
    P: ProcessSpawner + 'static,
    P::Process: 'static,
{
    async fn initialize(
        &self,
        _req: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        let mut caps_meta = serde_json::Map::new();
        caps_meta.insert("listChildren".to_string(), serde_json::json!({}));
        Ok(InitializeResponse::new(ProtocolVersion::LATEST)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .session_capabilities(
                        SessionCapabilities::new()
                            .fork(SessionForkCapabilities::new())
                            .list(SessionListCapabilities::new())
                            .resume(SessionResumeCapabilities::new())
                            .close(SessionCloseCapabilities::new())
                            .meta(caps_meta),
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
        let thread_id = proc
            .thread_start(&cwd)
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        drop(proc); // release process lock before acquiring sessions lock

        let session_id = Uuid::new_v4().to_string();
        self.sessions.lock().await.insert(
            session_id.clone(),
            CodexSession {
                thread_id,
                cwd,
                model: None,
                parent_session_id: None,
                history: Vec::new(),
                pending_history: None,
                first_turn: true,
            },
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
        let new_thread_id = proc
            .thread_fork(&source_thread_id)
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        drop(proc); // release process lock before acquiring sessions lock

        let new_session_id = Uuid::new_v4().to_string();
        self.sessions.lock().await.insert(
            new_session_id.clone(),
            CodexSession {
                thread_id: new_thread_id,
                cwd,
                model: inherited_model.clone(),
                parent_session_id: Some(source_id.clone()),
                history: Vec::new(),
                pending_history: None,
                first_turn: true,
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
            .map(|(id, s)| {
                let mut info = SessionInfo::new(id.clone(), s.cwd.clone());
                if let Some(ref parent_id) = s.parent_session_id {
                    let mut meta = serde_json::Map::new();
                    meta.insert("parentSessionId".to_string(), serde_json::json!(parent_id));
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

        if !self
            .available_models
            .iter()
            .any(|m| m.model_id.0.as_ref() == model_id)
        {
            return Err(invalid_params(format!("unknown model: {model_id}")));
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

    async fn prompt(&self, req: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        let session_id = req.session_id.to_string();
        let notifier = self.make_notifier(&req.session_id)?;

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
            warn!(
                session_id,
                "codex: prompt contains no text blocks; sending empty input to Codex"
            );
        }

        let (thread_id, model, pending_history) = {
            let mut sessions = self.sessions.lock().await;
            let s = sessions
                .get_mut(&session_id)
                .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
            let ph = s.pending_history.take();
            s.first_turn = false;
            s.history.push(trogon_runner_tools::portable_session::PortableMessage::text_only(
                "user", user_input.clone(),
            ));
            (s.thread_id.clone(), s.model.clone().or_else(|| Some(self.default_model.clone())), ph)
        };

        let user_input = if let Some(prior) = pending_history {
            let formatted = prior
                .iter()
                .map(|m| format!("[{}]: {}", m.role, m.text))
                .collect::<Vec<_>>()
                .join("\n");
            format!("Prior conversation:\n{formatted}\n\n---\n\n{user_input}")
        } else {
            user_input
        };

        let proc = self.process().await?;
        let mut event_rx = proc
            .turn_start(&thread_id, &user_input, model.as_deref())
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        drop(proc); // release process lock before entering the event loop

        // Stream Codex events → ACP SessionNotifications.
        let mut assistant_text = String::new();
        let mut tool_call_blocks: Vec<trogon_runner_tools::portable_session::PortableBlock> = Vec::new();
        let mut tool_result_blocks: Vec<trogon_runner_tools::portable_session::PortableBlock> = Vec::new();
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
                    assistant_text.push_str(&text);
                    let notif = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(
                            text,
                        ))),
                    );
                    if let Err(e) = notifier.session_notification(notif).await {
                        warn!(session_id, error = %e, "codex: failed to send text notification");
                    }
                }

                CodexEvent::ToolStarted { id, name, input } => {
                    let tool_call = ToolCall::new(id.clone(), name.clone())
                        .status(ToolCallStatus::InProgress)
                        .raw_input(input.clone())
                        .kind(ToolKind::Execute);
                    let notif = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::ToolCall(tool_call),
                    );
                    if let Err(e) = notifier.session_notification(notif).await {
                        warn!(session_id, error = %e, "codex: failed to send tool start notification");
                    }
                    tool_call_blocks.push(
                        trogon_runner_tools::portable_session::PortableBlock::ToolCall {
                            id,
                            name,
                            input,
                        },
                    );
                }

                CodexEvent::ToolCompleted { id, output } => {
                    let update = ToolCallUpdate::new(
                        id.clone(),
                        ToolCallUpdateFields::new()
                            .status(ToolCallStatus::Completed)
                            .raw_output(serde_json::Value::String(output.clone())),
                    );
                    let notif = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::ToolCallUpdate(update),
                    );
                    if let Err(e) = notifier.session_notification(notif).await {
                        warn!(session_id, error = %e, "codex: failed to send tool complete notification");
                    }
                    tool_result_blocks.push(
                        trogon_runner_tools::portable_session::PortableBlock::ToolResult {
                            tool_call_id: id,
                            content: output,
                        },
                    );
                }

                CodexEvent::TurnCompleted => {
                    let text = std::mem::take(&mut assistant_text);
                    let tool_calls = std::mem::take(&mut tool_call_blocks);
                    let tool_results = std::mem::take(&mut tool_result_blocks);
                    let mut sessions = self.sessions.lock().await;
                    if let Some(s) = sessions.get_mut(&session_id) {
                        if !tool_calls.is_empty() {
                            s.history.push(trogon_runner_tools::portable_session::PortableMessage {
                                role: "assistant".to_string(),
                                text: "[tool call]".to_string(),
                                blocks: tool_calls,
                            });
                        }
                        if !tool_results.is_empty() {
                            s.history.push(trogon_runner_tools::portable_session::PortableMessage {
                                role: "user".to_string(),
                                text: String::new(),
                                blocks: tool_results,
                            });
                        }
                        s.history.push(
                            trogon_runner_tools::portable_session::PortableMessage::text_only(
                                "assistant", text,
                            ),
                        );
                    }
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

    async fn cancel(&self, req: CancelNotification) -> agent_client_protocol::Result<()> {
        let session_id = req.session_id.to_string();

        let thread_id = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_id).map(|s| s.thread_id.clone())
        };

        // Only interrupt if the process is already alive — do not spawn a new
        // one just to cancel a turn that can't exist in a dead process.
        if let Some(thread_id) = thread_id {
            let guard = Arc::clone(&self.process).lock_owned().await;
            if let Some(p) = guard.as_ref()
                && p.is_alive()
                && let Err(e) = p.turn_interrupt(&thread_id).await
            {
                warn!(session_id, error = %e, "codex: turn_interrupt failed");
            }
        }

        Ok(())
    }

    async fn ext_method(&self, args: ExtRequest) -> agent_client_protocol::Result<ExtResponse> {
        if args.method.as_ref() == "session/list_children" {
            let params: serde_json::Value =
                serde_json::from_str(args.params.get()).unwrap_or_default();
            let parent_id = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let children: Vec<String> = self
                .sessions
                .lock()
                .await
                .iter()
                .filter(|(_, s)| s.parent_session_id.as_deref() == Some(parent_id))
                .map(|(id, _)| id.clone())
                .collect();
            let result = serde_json::json!({ "children": children });
            let raw = serde_json::value::RawValue::from_string(result.to_string())
                .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
            return Ok(ExtResponse::new(raw.into()));
        }
        if args.method.as_ref() == "session/get_state" {
            let params: serde_json::Value =
                serde_json::from_str(args.params.get()).unwrap_or_default();
            let session_id = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
            let sessions = self.sessions.lock().await;
            let state = sessions.get(session_id).ok_or_else(|| {
                Error::new(ErrorCode::InvalidParams.into(), "session not found")
            })?;
            let raw = serde_json::to_string(state)
                .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
            let raw = serde_json::value::RawValue::from_string(raw)
                .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
            return Ok(ExtResponse::new(raw.into()));
        }
        if args.method.as_ref() == "session/export" {
            let params: serde_json::Value =
                serde_json::from_str(args.params.get()).unwrap_or_default();
            let session_id = params["sessionId"].as_str()
                .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
            let sessions = self.sessions.lock().await;
            let s = sessions.get(session_id)
                .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "session not found"))?;
            let raw = serde_json::to_string(&s.history)
                .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
            return Ok(ExtResponse::new(serde_json::value::RawValue::from_string(raw)
                .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?.into()));
        }
        if args.method.as_ref() == "session/import" {
            let params: serde_json::Value =
                serde_json::from_str(args.params.get()).unwrap_or_default();
            let session_id = params["sessionId"].as_str()
                .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
            let messages: Vec<trogon_runner_tools::portable_session::PortableMessage> =
                serde_json::from_value(params["messages"].clone())
                    .map_err(|e| Error::new(ErrorCode::InvalidParams.into(), e.to_string()))?;
            let mut sessions = self.sessions.lock().await;
            let s = sessions.get_mut(session_id)
                .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "session not found"))?;
            s.history = messages.clone();
            s.pending_history = Some(messages);
            s.first_turn = true;
            let raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
            return Ok(ExtResponse::new(raw.into()));
        }
        Err(Error::new(
            ErrorCode::MethodNotFound.into(),
            format!("unknown ext method: {}", args.method),
        ))
    }
}

// ── Test helpers (same module scope → access to private fields) ───────────────

#[cfg(test)]
impl<N: SessionNotifierFactory, P: ProcessSpawner> CodexAgent<N, P> {
    /// Insert a session directly, bypassing the Codex subprocess.
    async fn test_insert_session(&self, id: &str, cwd: &str, model: Option<String>) {
        self.sessions.lock().await.insert(
            id.to_string(),
            CodexSession {
                thread_id: format!("thread-{id}"),
                cwd: cwd.to_string(),
                model,
                parent_session_id: None,
                history: Vec::new(),
                pending_history: None,
                first_turn: true,
            },
        );
    }

    async fn test_session_model(&self, id: &str) -> Option<String> {
        self.sessions
            .lock()
            .await
            .get(id)
            .and_then(|s| s.model.clone())
    }

    async fn test_session_parent_id(&self, id: &str) -> Option<String> {
        self.sessions
            .lock()
            .await
            .get(id)
            .and_then(|s| s.parent_session_id.clone())
    }

    async fn test_session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    fn test_prompt_timeout(&self) -> Duration {
        self.prompt_timeout
    }

    async fn test_get_history(
        &self,
        id: &str,
    ) -> Vec<trogon_runner_tools::portable_session::PortableMessage> {
        self.sessions
            .lock()
            .await
            .get(id)
            .map(|s| s.history.clone())
            .unwrap_or_default()
    }

    async fn test_get_pending_history(
        &self,
        id: &str,
    ) -> Option<Vec<trogon_runner_tools::portable_session::PortableMessage>> {
        self.sessions
            .lock()
            .await
            .get(id)
            .and_then(|s| s.pending_history.clone())
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        Agent, AuthMethodId, AuthenticateRequest, CancelNotification, CloseSessionRequest,
        ExtRequest, ForkSessionRequest, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
        PromptRequest, ProtocolVersion, ResumeSessionRequest, SetSessionConfigOptionRequest,
        SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModelRequest, StopReason,
    };
    use tokio::sync::broadcast;

    // ── In-memory mocks ───────────────────────────────────────────────────────

    struct MockSessionNotifier {
        recorded: Arc<Mutex<Vec<SessionNotification>>>,
    }

    #[async_trait(?Send)]
    impl SessionNotifier for MockSessionNotifier {
        async fn session_notification(
            &self,
            notif: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            self.recorded.lock().await.push(notif);
            Ok(())
        }
    }

    struct MockNotifierFactory {
        recorded: Arc<Mutex<Vec<SessionNotification>>>,
    }

    impl MockNotifierFactory {
        fn new() -> Self {
            Self {
                recorded: Arc::new(Mutex::new(vec![])),
            }
        }
    }

    impl SessionNotifierFactory for MockNotifierFactory {
        type Notifier = MockSessionNotifier;

        fn make_notifier(
            &self,
            _session_id: &SessionId,
        ) -> agent_client_protocol::Result<MockSessionNotifier> {
            Ok(MockSessionNotifier {
                recorded: Arc::clone(&self.recorded),
            })
        }
    }

    struct MockCodexProcess {
        events: Vec<CodexEvent>,
    }

    #[async_trait(?Send)]
    impl CodexProcessClient for MockCodexProcess {
        fn is_alive(&self) -> bool {
            true
        }

        async fn thread_start(
            &self,
            _cwd: &str,
        ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Ok("mock-thread-id".to_string())
        }

        async fn thread_resume(
            &self,
            thread_id: &str,
        ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Ok(thread_id.to_string())
        }

        async fn thread_fork(
            &self,
            _thread_id: &str,
        ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Ok(format!("fork-{}", Uuid::new_v4()))
        }

        async fn turn_start(
            &self,
            _thread_id: &str,
            _user_input: &str,
            _model: Option<&str>,
        ) -> Result<broadcast::Receiver<CodexEvent>, Box<dyn std::error::Error + Send + Sync>>
        {
            let (tx, rx) = broadcast::channel(64);
            for event in &self.events {
                let _ = tx.send(event.clone());
            }
            Ok(rx)
        }

        async fn turn_interrupt(
            &self,
            _thread_id: &str,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }
    }

    struct MockProcessSpawner {
        events: Vec<CodexEvent>,
    }

    impl MockProcessSpawner {
        fn new() -> Self {
            Self { events: vec![] }
        }
    }

    #[async_trait(?Send)]
    impl ProcessSpawner for MockProcessSpawner {
        type Process = MockCodexProcess;

        async fn spawn(
            &self,
        ) -> Result<MockCodexProcess, Box<dyn std::error::Error + Send + Sync>> {
            Ok(MockCodexProcess {
                events: self.events.clone(),
            })
        }
    }

    async fn make_agent() -> CodexAgent<MockNotifierFactory, MockProcessSpawner> {
        CodexAgent::new(
            MockNotifierFactory::new(),
            MockProcessSpawner::new(),
            "o4-mini",
        )
    }

    // ── close_session ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn close_session_removes_session() {
        let agent = make_agent().await;
        agent.test_insert_session("s1", "/tmp", None).await;
        assert_eq!(agent.test_session_count().await, 1);

        agent
            .close_session(CloseSessionRequest::new("s1"))
            .await
            .unwrap();
        assert_eq!(agent.test_session_count().await, 0);
    }

    #[tokio::test]
    async fn close_session_unknown_id_is_noop() {
        let agent = make_agent().await;
        // Must not return an error for unknown session ids.
        agent
            .close_session(CloseSessionRequest::new("nonexistent"))
            .await
            .unwrap();
    }

    // ── load_session ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn load_session_returns_state() {
        let agent = make_agent().await;
        agent
            .test_insert_session("s2", "/home/user", Some("o3".to_string()))
            .await;

        let resp = agent
            .load_session(LoadSessionRequest::new("s2", "/home/user"))
            .await
            .unwrap();
        assert_eq!(resp.models.unwrap().current_model_id.to_string(), "o3");
    }

    #[tokio::test]
    async fn load_session_not_found_returns_error() {
        let agent = make_agent().await;
        assert!(
            agent
                .load_session(LoadSessionRequest::new("missing", "/"))
                .await
                .is_err()
        );
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
        assert!(
            agent
                .set_session_model(SetSessionModelRequest::new("missing", "o3"))
                .await
                .is_err()
        );
    }

    // ── list_sessions ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_returns_sorted() {
        let agent = make_agent().await;
        agent.test_insert_session("zzz", "/c", None).await;
        agent.test_insert_session("aaa", "/a", None).await;
        agent.test_insert_session("mmm", "/b", None).await;

        let resp = agent
            .list_sessions(ListSessionsRequest::new())
            .await
            .unwrap();
        let ids: Vec<_> = resp
            .sessions
            .iter()
            .map(|s| s.session_id.to_string())
            .collect();
        assert_eq!(ids, vec!["aaa", "mmm", "zzz"]);
    }

    #[tokio::test]
    async fn list_sessions_empty() {
        let agent = make_agent().await;
        let resp = agent
            .list_sessions(ListSessionsRequest::new())
            .await
            .unwrap();
        assert!(resp.sessions.is_empty());
    }

    // ── default_model validation ──────────────────────────────────────────────

    #[tokio::test]
    async fn default_model_added_when_not_in_list() {
        let agent = CodexAgent::new(
            MockNotifierFactory::new(),
            MockProcessSpawner::new(),
            "custom-model",
        );

        // session_model_state should include "custom-model" in available list.
        let state = agent.session_model_state(None);
        let ids: Vec<_> = state
            .available_models
            .iter()
            .map(|m| m.model_id.to_string())
            .collect();
        assert!(
            ids.contains(&"custom-model".to_string()),
            "available: {ids:?}"
        );
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
        assert!(
            resp.agent_capabilities.load_session,
            "load_session should be true"
        );
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
        assert!(
            sc.resume.is_some(),
            "resume capability should be advertised"
        );
        assert!(sc.close.is_some(), "close capability should be advertised");
        let meta = sc.meta.expect("session_capabilities must have _meta");
        assert!(
            meta.contains_key("listChildren"),
            "caps _meta must advertise listChildren"
        );
        assert!(
            !meta.contains_key("branchAtIndex"),
            "branchAtIndex must not be advertised (not supported)"
        );
    }

    // ── authenticate ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn authenticate_returns_ok() {
        let agent = make_agent().await;
        agent
            .authenticate(AuthenticateRequest::new(AuthMethodId::from("any")))
            .await
            .unwrap();
    }

    // ── set_session_mode ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_mode_always_succeeds() {
        let agent = make_agent().await;
        agent.test_insert_session("sm1", "/tmp", None).await;
        agent
            .set_session_mode(SetSessionModeRequest::new("sm1", "default"))
            .await
            .unwrap();
    }

    // ── set_session_config_option ─────────────────────────────────────────────

    #[tokio::test]
    async fn set_session_config_option_returns_empty_list() {
        let agent = make_agent().await;
        let resp = agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("s1", "key", "value"))
            .await
            .unwrap();
        assert_eq!(resp, SetSessionConfigOptionResponse::new(vec![]));
    }

    // ── prompt_timeout env var ────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_timeout_defaults_to_7200s() {
        unsafe { std::env::remove_var("CODEX_PROMPT_TIMEOUT_SECS") };
        let agent = make_agent().await;
        assert_eq!(agent.test_prompt_timeout(), Duration::from_secs(7200));
    }

    // ── broadcast channel helpers ─────────────────────────────────────────────

    /// Verifies that a lagged broadcast receiver can continue receiving after lag.
    #[tokio::test]
    async fn broadcast_channel_lag_allows_recovery() {
        use tokio::sync::broadcast;
        let (tx, mut rx) = broadcast::channel::<i32>(2);
        // Send 3 items into a capacity-2 channel without reading — forces lag.
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();

        // First recv must return Lagged (not a value).
        match rx.recv().await {
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            other => panic!("expected Lagged, got {:?}", other),
        }
        let v = rx.recv().await.unwrap();
        assert_eq!(v, 2);
        let v = rx.recv().await.unwrap();
        assert_eq!(v, 3);
    }

    #[tokio::test]
    async fn broadcast_channel_closed_returns_closed_error() {
        use tokio::sync::broadcast;
        let (tx, mut rx) = broadcast::channel::<i32>(8);
        drop(tx);
        match rx.recv().await {
            Err(broadcast::error::RecvError::Closed) => {}
            other => panic!("expected Closed, got {:?}", other),
        }
    }

    // ── resume_session ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resume_session_succeeds_for_existing_session() {
        let agent = make_agent().await;
        agent.test_insert_session("r1", "/tmp", None).await;
        agent
            .resume_session(ResumeSessionRequest::new("r1", "/tmp"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resume_session_not_found_returns_error() {
        let agent = make_agent().await;
        assert!(
            agent
                .resume_session(ResumeSessionRequest::new("missing", "/"))
                .await
                .is_err()
        );
    }

    // ── fork_session ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fork_session_creates_new_session() {
        let agent = make_agent().await;
        agent.test_insert_session("f1", "/src", None).await;
        let resp = agent
            .fork_session(ForkSessionRequest::new("f1", "/src"))
            .await
            .unwrap();
        assert!(!resp.session_id.to_string().is_empty());
        assert_eq!(agent.test_session_count().await, 2);
    }

    #[tokio::test]
    async fn fork_session_not_found_returns_error() {
        let agent = make_agent().await;
        assert!(
            agent
                .fork_session(ForkSessionRequest::new("missing", "/src"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn fork_session_records_parent_id() {
        let agent = make_agent().await;
        agent.test_insert_session("f2", "/src", None).await;
        let resp = agent
            .fork_session(ForkSessionRequest::new("f2", "/fork"))
            .await
            .unwrap();
        let new_id = resp.session_id.to_string();
        assert_eq!(
            agent.test_session_parent_id(&new_id).await.as_deref(),
            Some("f2"),
            "fork must record parent session ID"
        );
    }

    #[tokio::test]
    async fn list_sessions_branch_has_parent_meta() {
        let agent = make_agent().await;
        agent.test_insert_session("src", "/root", None).await;
        let resp = agent
            .fork_session(ForkSessionRequest::new("src", "/branch"))
            .await
            .unwrap();
        let fork_id = resp.session_id.to_string();

        let list_resp = agent
            .list_sessions(ListSessionsRequest::new())
            .await
            .unwrap();

        let branch_info = list_resp
            .sessions
            .iter()
            .find(|s| s.session_id.to_string() == fork_id)
            .expect("forked session must appear in list");
        let meta = branch_info.meta.as_ref().expect("branch must have _meta");
        assert_eq!(
            meta.get("parentSessionId").and_then(|v| v.as_str()),
            Some("src"),
            "parentSessionId must be in _meta"
        );

        let root_info = list_resp
            .sessions
            .iter()
            .find(|s| s.session_id.to_string() == "src")
            .expect("root must appear in list");
        assert!(root_info.meta.is_none(), "root must not have branch _meta");
    }

    /// `branchAtIndex` is not supported by codex-runner (Codex manages its own
    /// history inside the subprocess). Passing it must not cause an error and
    /// must not surface as `branchedAtIndex` in `list_sessions._meta`.
    #[tokio::test]
    async fn fork_session_silently_ignores_branch_at_index() {
        let agent = make_agent().await;
        agent.test_insert_session("src", "/root", None).await;

        let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
            serde_json::json!({ "branchAtIndex": 2 }),
        )
        .unwrap();
        let fork_id = agent
            .fork_session(ForkSessionRequest::new("src", "/branch").meta(meta))
            .await
            .unwrap()
            .session_id
            .to_string();

        let list_resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
        let fork_info = list_resp
            .sessions
            .iter()
            .find(|s| s.session_id.to_string() == fork_id)
            .expect("forked session must appear in list");
        assert!(
            fork_info
                .meta
                .as_ref()
                .map_or(true, |m| !m.contains_key("branchedAtIndex")),
            "branchedAtIndex must not appear in _meta — branchAtIndex is not supported"
        );
    }

    // ── ext_method / session/list_children ───────────────────────────────────

    #[tokio::test]
    async fn ext_list_children_returns_direct_children() {
        let agent = make_agent().await;
        agent.test_insert_session("parent", "/tmp", None).await;
        agent.test_insert_session("other", "/tmp", None).await;

        let resp1 = agent
            .fork_session(ForkSessionRequest::new("parent", "/b1"))
            .await
            .unwrap();
        let child1 = resp1.session_id.to_string();
        let resp2 = agent
            .fork_session(ForkSessionRequest::new("parent", "/b2"))
            .await
            .unwrap();
        let child2 = resp2.session_id.to_string();

        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "parent" }).to_string(),
        )
        .unwrap();
        let ext_req = ExtRequest::new("session/list_children", raw_params.into());
        let resp = agent.ext_method(ext_req).await.unwrap();
        let result: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
        let mut children: Vec<String> = result["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        children.sort();
        let mut expected = vec![child1, child2];
        expected.sort();
        assert_eq!(children, expected);
    }

    #[tokio::test]
    async fn ext_list_children_returns_empty_for_root_session() {
        let agent = make_agent().await;
        agent.test_insert_session("root", "/tmp", None).await;

        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "root" }).to_string(),
        )
        .unwrap();
        let ext_req = ExtRequest::new("session/list_children", raw_params.into());
        let resp = agent.ext_method(ext_req).await.unwrap();
        let result: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
        assert_eq!(result["children"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn ext_unknown_method_returns_method_not_found() {
        let agent = make_agent().await;
        let raw_params =
            serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let ext_req = ExtRequest::new("session/unknown", raw_params.into());
        let err = agent.ext_method(ext_req).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::MethodNotFound);
    }

    #[tokio::test]
    async fn ext_list_children_only_returns_direct_children_not_grandchildren() {
        let agent = make_agent().await;

        // A → B → C
        agent.test_insert_session("a", "/tmp", None).await;
        let b = agent
            .fork_session(ForkSessionRequest::new("a", "/b"))
            .await
            .unwrap()
            .session_id
            .to_string();
        let c = agent
            .fork_session(ForkSessionRequest::new(b.clone(), "/c"))
            .await
            .unwrap()
            .session_id
            .to_string();

        let list_children = |sid: String| {
            let raw = serde_json::value::RawValue::from_string(
                serde_json::json!({ "sessionId": sid }).to_string(),
            )
            .unwrap();
            ExtRequest::new("session/list_children", raw.into())
        };
        let parse = |resp: ExtResponse| -> Vec<String> {
            let v: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
            v["children"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect()
        };

        let children_a = parse(agent.ext_method(list_children("a".to_string())).await.unwrap());
        assert_eq!(children_a, vec![b.clone()], "A must have only B as child");

        let children_b = parse(agent.ext_method(list_children(b.clone())).await.unwrap());
        assert_eq!(children_b, vec![c.clone()], "B must have only C as child");

        let children_c = parse(agent.ext_method(list_children(c.clone())).await.unwrap());
        assert!(children_c.is_empty(), "C must have no children");
    }

    // ── ext_method / session/get_state ───────────────────────────────────────

    #[tokio::test]
    async fn ext_get_state_returns_session_json() {
        let agent = make_agent().await;
        agent.test_insert_session("gs1", "/projects/myapp", None).await;

        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "gs1" }).to_string(),
        )
        .unwrap();
        let resp = agent.ext_method(ExtRequest::new("session/get_state", raw_params.into())).await.unwrap();
        let state: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
        assert_eq!(state["cwd"].as_str(), Some("/projects/myapp"), "cwd must match inserted session");
    }

    #[tokio::test]
    async fn ext_get_state_includes_thread_id() {
        let agent = make_agent().await;
        agent.test_insert_session("ti1", "/tmp", None).await;

        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "ti1" }).to_string(),
        )
        .unwrap();
        let resp = agent.ext_method(ExtRequest::new("session/get_state", raw_params.into())).await.unwrap();
        let state: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
        assert_eq!(
            state["thread_id"].as_str(),
            Some("thread-ti1"),
            "thread_id must be present in get_state (test helper sets thread-{{id}})"
        );
    }

    #[tokio::test]
    async fn ext_get_state_returns_model_when_set() {
        let agent = make_agent().await;
        agent.test_insert_session("ms1", "/tmp", Some("o4-mini".to_string())).await;

        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "ms1" }).to_string(),
        )
        .unwrap();
        let resp = agent.ext_method(ExtRequest::new("session/get_state", raw_params.into())).await.unwrap();
        let state: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
        assert_eq!(
            state["model"].as_str(),
            Some("o4-mini"),
            "model override must appear in get_state (needed by /model command)"
        );
    }

    #[tokio::test]
    async fn ext_get_state_missing_session_id_returns_invalid_params() {
        let agent = make_agent().await;
        let raw_params = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let err = agent.ext_method(ExtRequest::new("session/get_state", raw_params.into())).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn ext_get_state_unknown_session_id_returns_invalid_params() {
        let agent = make_agent().await;
        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "nonexistent" }).to_string(),
        )
        .unwrap();
        let err = agent.ext_method(ExtRequest::new("session/get_state", raw_params.into())).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    // ── cancel ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_unknown_session_is_noop() {
        let agent = make_agent().await;
        agent
            .cancel(CancelNotification::new("nonexistent"))
            .await
            .unwrap();
    }

    // ── prompt ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_with_no_events_returns_end_turn() {
        let agent = make_agent().await;
        agent.test_insert_session("p1", "/tmp", None).await;
        let resp = agent
            .prompt(PromptRequest::new("p1", vec![ContentBlock::from("hello")]))
            .await
            .unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn prompt_streams_text_events_as_notifications() {
        let factory = MockNotifierFactory::new();
        let recorded = Arc::clone(&factory.recorded);
        let spawner = MockProcessSpawner {
            events: vec![
                CodexEvent::TextDelta {
                    text: "hello".to_string(),
                },
                CodexEvent::TurnCompleted,
            ],
        };
        let agent = CodexAgent::new(factory, spawner, "o4-mini");
        agent.test_insert_session("p2", "/tmp", None).await;

        agent
            .prompt(PromptRequest::new("p2", vec![ContentBlock::from("go")]))
            .await
            .unwrap();

        let notifs = recorded.lock().await;
        assert_eq!(notifs.len(), 1, "expected one text notification");
    }

    // ── ext_method / session/export ───────────────────────────────────────────

    #[tokio::test]
    async fn ext_method_export_returns_serialized_history() {
        use trogon_runner_tools::portable_session::PortableMessage;
        let agent = make_agent().await;
        agent.test_insert_session("e1", "/tmp", None).await;
        agent
            .sessions
            .lock()
            .await
            .get_mut("e1")
            .unwrap()
            .history
            .push(PortableMessage::text_only("user", "hello"));

        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "e1" }).to_string(),
        )
        .unwrap();
        let ext_req = ExtRequest::new("session/export", raw_params.into());
        let resp = agent.ext_method(ext_req).await.unwrap();
        let msgs: Vec<PortableMessage> = serde_json::from_str(resp.0.get()).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hello");
        assert_eq!(msgs[0].role, "user");
    }

    #[tokio::test]
    async fn ext_method_export_unknown_session_returns_error() {
        let agent = make_agent().await;
        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "no-such" }).to_string(),
        )
        .unwrap();
        let ext_req = ExtRequest::new("session/export", raw_params.into());
        assert!(agent.ext_method(ext_req).await.is_err());
    }

    #[tokio::test]
    async fn ext_method_export_missing_session_id_returns_error() {
        let agent = make_agent().await;
        let raw_params =
            serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let ext_req = ExtRequest::new("session/export", raw_params.into());
        assert!(agent.ext_method(ext_req).await.is_err());
    }

    #[tokio::test]
    async fn export_after_real_turn_contains_accumulated_history() {
        use trogon_runner_tools::portable_session::PortableMessage;
        let spawner = MockProcessSpawner {
            events: vec![
                CodexEvent::TextDelta {
                    text: "hello".to_string(),
                },
                CodexEvent::TextDelta {
                    text: " world".to_string(),
                },
                CodexEvent::TurnCompleted,
            ],
        };
        let agent = CodexAgent::new(MockNotifierFactory::new(), spawner, "o4-mini");
        agent.test_insert_session("ep1", "/tmp", None).await;

        agent
            .prompt(PromptRequest::new(
                "ep1",
                vec![ContentBlock::from("question".to_string())],
            ))
            .await
            .unwrap();

        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "ep1" }).to_string(),
        )
        .unwrap();
        let ext_req = ExtRequest::new("session/export", raw_params.into());
        let resp = agent.ext_method(ext_req).await.unwrap();
        let msgs: Vec<PortableMessage> = serde_json::from_str(resp.0.get()).unwrap();

        assert_eq!(msgs.len(), 2, "expected user + assistant messages");
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "question");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].text, "hello world");
    }

    // ── ext_method / session/import ───────────────────────────────────────────

    #[tokio::test]
    async fn ext_method_import_sets_pending_history_and_first_turn() {
        let agent = make_agent().await;
        agent.test_insert_session("i1", "/tmp", None).await;
        // Simulate that first_turn was already consumed.
        agent
            .sessions
            .lock()
            .await
            .get_mut("i1")
            .unwrap()
            .first_turn = false;

        let raw_params = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "sessionId": "i1",
                "messages": [{ "role": "user", "text": "ctx" }]
            })
            .to_string(),
        )
        .unwrap();
        let ext_req = ExtRequest::new("session/import", raw_params.into());
        agent.ext_method(ext_req).await.unwrap();

        let pending = agent.test_get_pending_history("i1").await;
        assert!(pending.is_some(), "pending_history must be set after import");
        let msgs = pending.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "ctx");

        // import must also write to s.history for export consistency
        let history = agent.test_get_history("i1").await;
        assert_eq!(history.len(), 1, "s.history must mirror imported messages");
        assert_eq!(history[0].text, "ctx");

        // import must reset first_turn to true
        let first_turn = agent
            .sessions
            .lock()
            .await
            .get("i1")
            .unwrap()
            .first_turn;
        assert!(first_turn, "import must reset first_turn to true");
    }

    // ── ext_method / export→import round-trip ────────────────────────────────

    #[tokio::test]
    async fn ext_method_export_import_round_trip() {
        use trogon_runner_tools::portable_session::PortableMessage;
        let agent = make_agent().await;

        // Build source session with two history messages.
        agent.test_insert_session("src", "/tmp", None).await;
        {
            let mut sessions = agent.sessions.lock().await;
            let src = sessions.get_mut("src").unwrap();
            src.history.push(PortableMessage::text_only("user", "q"));
            src.history.push(PortableMessage::text_only("assistant", "a"));
        }

        // Export from src.
        let export_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "src" }).to_string(),
        )
        .unwrap();
        let export_resp = agent
            .ext_method(ExtRequest::new("session/export", export_params.into()))
            .await
            .unwrap();
        let exported_json = export_resp.0.get().to_string();

        // Import into dst.
        agent.test_insert_session("dst", "/tmp", None).await;
        let import_params = serde_json::value::RawValue::from_string(format!(
            r#"{{"sessionId":"dst","messages":{exported_json}}}"#
        ))
        .unwrap();
        agent
            .ext_method(ExtRequest::new("session/import", import_params.into()))
            .await
            .unwrap();

        // Re-export from dst: must return the imported messages immediately
        // (before any prompt) — consistent with xai/openrouter/acp behaviour.
        let export_dst_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": "dst" }).to_string(),
        )
        .unwrap();
        let export_dst_resp = agent
            .ext_method(ExtRequest::new("session/export", export_dst_params.into()))
            .await
            .unwrap();
        let dst_portable: Vec<trogon_runner_tools::portable_session::PortableMessage> =
            serde_json::from_str(export_dst_resp.0.get()).unwrap();
        assert_eq!(dst_portable.len(), 2, "export after import must return imported messages");
        assert_eq!(dst_portable[0].role, "user");
        assert_eq!(dst_portable[0].text, "q");
        assert_eq!(dst_portable[1].role, "assistant");
        assert_eq!(dst_portable[1].text, "a");

        // pending_history must also be set (for subprocess injection on first prompt).
        let pending = agent.test_get_pending_history("dst").await.unwrap();
        assert_eq!(pending.len(), 2, "pending_history must also be set for subprocess injection");
    }

    // ── history accumulation ──────────────────────────────────────────────────

    #[tokio::test]
    async fn history_accumulation_user_message_pushed_before_turn() {
        let spawner = MockProcessSpawner {
            events: vec![CodexEvent::TurnCompleted],
        };
        let agent = CodexAgent::new(MockNotifierFactory::new(), spawner, "o4-mini");
        agent.test_insert_session("p1", "/tmp", None).await;

        agent
            .prompt(PromptRequest::new("p1", vec![ContentBlock::from("hello")]))
            .await
            .unwrap();

        let history = agent.test_get_history("p1").await;
        assert!(!history.is_empty(), "history must not be empty after prompt");
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].text, "hello");
    }

    #[tokio::test]
    async fn history_accumulation_assistant_message_pushed_on_turn_completed() {
        let spawner = MockProcessSpawner {
            events: vec![
                CodexEvent::TextDelta {
                    text: "world".to_string(),
                },
                CodexEvent::TurnCompleted,
            ],
        };
        let agent = CodexAgent::new(MockNotifierFactory::new(), spawner, "o4-mini");
        agent.test_insert_session("p2", "/tmp", None).await;

        agent
            .prompt(PromptRequest::new("p2", vec![ContentBlock::from("hello")]))
            .await
            .unwrap();

        let history = agent.test_get_history("p2").await;
        assert_eq!(history.len(), 2, "expected user + assistant messages");
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].text, "hello");
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[1].text, "world");
    }

    // ── ext_method / session/import — error cases ─────────────────────────────

    #[tokio::test]
    async fn ext_method_import_unknown_session_returns_error() {
        let agent = make_agent().await;
        // no sessions inserted
        let params = serde_json::value::RawValue::from_string(
            serde_json::json!({"sessionId":"no-such","messages":[]}).to_string()
        ).unwrap();
        let result = agent.ext_method(ExtRequest::new("session/import", params.into())).await;
        assert!(result.is_err(), "import of unknown session must return Err");
    }

    // ── history: pending prepend on import then prompt ────────────────────────
    //
    // Test 8: After import, the pending_history is prepended to the user input
    // with "Prior conversation:" formatting before it is sent to the mock process.
    // Since MockCodexProcess does not capture the input it receives, we verify
    // the side-effect instead: pending_history is consumed (None) after the prompt
    // and both user + assistant messages appear correctly in session history.
    #[tokio::test]
    async fn history_pending_prepend_on_import_then_prompt() {
        let spawner = MockProcessSpawner {
            events: vec![
                CodexEvent::TextDelta {
                    text: "response".to_string(),
                },
                CodexEvent::TurnCompleted,
            ],
        };
        let agent = CodexAgent::new(MockNotifierFactory::new(), spawner, "o4-mini");
        agent.test_insert_session("ip1", "/tmp", None).await;

        // Import prior context.
        let import_params = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "sessionId": "ip1",
                "messages": [{ "role": "user", "text": "prior context" }]
            })
            .to_string(),
        )
        .unwrap();
        agent
            .ext_method(ExtRequest::new("session/import", import_params.into()))
            .await
            .unwrap();

        // Verify pending_history is set before the prompt.
        assert!(
            agent.test_get_pending_history("ip1").await.is_some(),
            "pending_history must be set before prompt"
        );

        // Drive a prompt — the agent prepends "Prior conversation:\n..." to the
        // user input internally before handing it to the process.
        agent
            .prompt(PromptRequest::new(
                "ip1",
                vec![ContentBlock::from("actual question")],
            ))
            .await
            .unwrap();

        // After the prompt, pending_history must have been consumed.
        assert!(
            agent.test_get_pending_history("ip1").await.is_none(),
            "pending_history must be None after prompt consumed it"
        );

        // History should contain the imported message + user message + assistant reply.
        // s.history is written on import so export is consistent; prompt appends
        // the new user/assistant turn on top of the already-stored imported messages.
        let history = agent.test_get_history("ip1").await;
        assert_eq!(history.len(), 3, "expected imported + user + assistant messages");
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].text, "prior context");
        assert_eq!(history[1].role, "user");
        assert_eq!(history[1].text, "actual question");
        assert_eq!(history[2].role, "assistant");
        assert_eq!(history[2].text, "response");
    }
}
