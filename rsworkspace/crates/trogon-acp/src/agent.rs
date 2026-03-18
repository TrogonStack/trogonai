//! `TrogonAcpAgent` — local implementation of the ACP [`Agent`] trait.
//!
//! Handles all lifecycle methods locally.  Delegates `prompt` and `cancel`
//! to the inner [`Bridge`], which routes them through NATS to the Runner.

use std::path::PathBuf;
use std::time::Duration;

use agent_client_protocol::{
    AgentCapabilities, AuthenticateRequest, AuthenticateResponse, AvailableCommandsUpdate,
    CancelNotification, ConfigOptionUpdate, ContentBlock, ContentChunk, CurrentModeUpdate, Error,
    ErrorCode, ExtNotification, ExtRequest, ExtResponse, ForkSessionRequest, ForkSessionResponse,
    Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, McpCapabilities, ModelInfo,
    NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
    ProtocolVersion, Result, ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities,
    SessionConfigOption, SessionConfigOptionCategory, SessionForkCapabilities, SessionId,
    SessionInfo, SessionListCapabilities, SessionMode, SessionModeState, SessionModelState,
    SessionNotification, SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModelRequest, SetSessionModelResponse,
    SetSessionModeRequest, SetSessionModeResponse, TextContent, ToolCall, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

use acp_nats::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use acp_nats::Bridge;
use trogon_acp_runner::{SessionState, SessionStore};
use trogon_agent::agent_loop::ContentBlock as AgentContentBlock;
use trogon_std::time::GetElapsed;

const SESSION_READY_DELAY: Duration = Duration::from_millis(100);

/// Hardcoded available Claude models exposed by this agent.
const AVAILABLE_MODELS: &[(&str, &str)] = &[
    ("claude-opus-4-6", "Claude Opus 4"),
    ("claude-sonnet-4-6", "Claude Sonnet 4"),
    ("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
];

/// ACP `Agent` implementation that handles lifecycle methods locally and
/// routes `prompt`/`cancel` through NATS via the inner `Bridge`.
pub struct TrogonAcpAgent<N, C>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
{
    pub(crate) bridge: Bridge<N, C>,
    pub(crate) store: SessionStore,
    pub(crate) nats: async_nats::Client,
    pub(crate) prefix: String,
    pub(crate) notification_sender: mpsc::Sender<SessionNotification>,
    /// Default model configured for this agent instance (from AGENT_MODEL env var).
    pub(crate) default_model: String,
}

impl<N, C> TrogonAcpAgent<N, C>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
{
    pub fn new(
        bridge: Bridge<N, C>,
        store: SessionStore,
        nats: async_nats::Client,
        prefix: impl Into<String>,
        notification_sender: mpsc::Sender<SessionNotification>,
        default_model: impl Into<String>,
    ) -> Self {
        Self {
            bridge,
            store,
            nats,
            prefix: prefix.into(),
            notification_sender,
            default_model: default_model.into(),
        }
    }

    /// Build the `SessionModeState` for a session.
    fn build_mode_state(current_mode: &str) -> SessionModeState {
        SessionModeState::new(
            current_mode.to_string(),
            vec![
                SessionMode::new("default", "Default")
                    .description("Standard behavior"),
                SessionMode::new("acceptEdits", "Accept Edits")
                    .description("Auto-accept file edit operations"),
                SessionMode::new("plan", "Plan Mode")
                    .description("Planning mode, no actual tool execution"),
                SessionMode::new("dontAsk", "Don't Ask")
                    .description("Don't prompt for permissions"),
            ],
        )
    }

    /// Build the `SessionModelState` for a session.
    fn build_model_state(current_model: &str) -> SessionModelState {
        let available = AVAILABLE_MODELS
            .iter()
            .map(|(id, name)| ModelInfo::new(*id, *name))
            .collect();
        SessionModelState::new(current_model.to_string(), available)
    }

    /// Build the `SessionConfigOption` list for a session.
    fn build_config_options(current_mode: &str, current_model: &str) -> Vec<SessionConfigOption> {
        use agent_client_protocol::SessionConfigSelectOption;
        let mode_options: Vec<SessionConfigSelectOption> = vec![
            SessionConfigSelectOption::new("default", "Default"),
            SessionConfigSelectOption::new("acceptEdits", "Accept Edits"),
            SessionConfigSelectOption::new("plan", "Plan Mode"),
            SessionConfigSelectOption::new("dontAsk", "Don't Ask"),
        ];
        let model_options: Vec<SessionConfigSelectOption> = AVAILABLE_MODELS
            .iter()
            .map(|(id, name)| SessionConfigSelectOption::new(*id, *name))
            .collect();

        vec![
            SessionConfigOption::select("mode", "Mode", current_mode.to_string(), mode_options)
                .category(SessionConfigOptionCategory::Mode),
            SessionConfigOption::select("model", "Model", current_model.to_string(), model_options)
                .category(SessionConfigOptionCategory::Model),
        ]
    }

    async fn publish_session_ready(&self, session_id: &str) {
        let nats = self.nats.clone();
        let subject = format!("{}.{}.agent.ext.session.ready", self.prefix, session_id);
        let body = serde_json::to_vec(&serde_json::json!({ "sessionId": session_id }))
            .unwrap_or_default();

        tokio::spawn(async move {
            tokio::time::sleep(SESSION_READY_DELAY).await;
            if let Err(e) = nats.publish(subject.clone(), body.into()).await {
                warn!(subject = %subject, error = %e, "Failed to publish session.ready");
            }
        });
    }

    /// Send an `available_commands_update` notification asynchronously.
    /// We have no slash commands in the NATS model, so we send an empty list.
    async fn send_available_commands_update(&self, session_id: &SessionId) {
        let notification = SessionNotification::new(
            session_id.clone(),
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(vec![])),
        );
        let sender = self.notification_sender.clone();
        let sid = session_id.clone();
        tokio::spawn(async move {
            if sender.send(notification).await.is_err() {
                warn!(session_id = %sid, "notification receiver dropped sending available_commands");
            }
        });
    }

    /// Replay session history as ACP notifications.
    ///
    /// - User messages (simple text): skipped
    /// - Assistant text: `AgentMessageChunk`
    /// - Assistant tool_use: `ToolCall` (InProgress → Completed)
    /// - User tool_result: `ToolCallUpdate` (Completed)
    async fn replay_history(&self, session_id: &SessionId, state: &SessionState) {
        for msg in &state.messages {
            match msg.role.as_str() {
                "assistant" => {
                    for block in &msg.content {
                        match block {
                            AgentContentBlock::Text { text } if !text.is_empty() => {
                                let n = SessionNotification::new(
                                    session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(text.clone())),
                                    )),
                                );
                                if self.notification_sender.send(n).await.is_err() {
                                    return;
                                }
                            }
                            AgentContentBlock::ToolUse { id, name, input } => {
                                // Show as InProgress then immediately Completed
                                let tool_call = ToolCall::new(id.clone(), name.clone())
                                    .status(ToolCallStatus::InProgress)
                                    .raw_input(input.clone());
                                let n = SessionNotification::new(
                                    session_id.clone(),
                                    SessionUpdate::ToolCall(tool_call),
                                );
                                if self.notification_sender.send(n).await.is_err() {
                                    return;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "user" => {
                    for block in &msg.content {
                        if let AgentContentBlock::ToolResult { tool_use_id, content } = block {
                            let fields = ToolCallUpdateFields::new()
                                .status(ToolCallStatus::Completed)
                                .raw_output(serde_json::Value::String(content.clone()));
                            let update = ToolCallUpdate::new(tool_use_id.clone(), fields);
                            let n = SessionNotification::new(
                                session_id.clone(),
                                SessionUpdate::ToolCallUpdate(update),
                            );
                            if self.notification_sender.send(n).await.is_err() {
                                return;
                            }
                        }
                        // Simple user text messages are skipped (matching TS behaviour)
                    }
                }
                _ => {}
            }
        }
    }

    /// Delete a session from KV and publish a cancel to abort any running prompt.
    async fn close_session_impl(&self, session_id: &str) {
        let cancel_subject =
            acp_nats::nats::agent::session_cancel(&self.prefix, session_id);
        let empty: Vec<u8> = vec![];
        let _ = self.nats.publish(cancel_subject, empty.into()).await;
        if let Err(e) = self.store.delete(session_id).await {
            warn!(session_id, error = %e, "Failed to delete session on close");
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<N, C> agent_client_protocol::Agent for TrogonAcpAgent<N, C>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient + Clone + Send + Sync + 'static,
    C: GetElapsed + Send + Sync + 'static,
{
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        let client = args
            .client_info
            .as_ref()
            .map(|c| c.name.as_str())
            .unwrap_or("unknown");
        info!(client = %client, "ACP initialize");

        let mut caps_meta = serde_json::Map::new();
        // Advertise `close` capability — not yet a first-class field in the Rust SDK
        caps_meta.insert("close".to_string(), serde_json::json!({}));

        let session_caps = SessionCapabilities::new()
            .list(SessionListCapabilities::new())
            .fork(SessionForkCapabilities::new())
            .resume(SessionResumeCapabilities::new())
            .meta(caps_meta);

        let mut meta = serde_json::Map::new();
        meta.insert(
            "claudeCode".to_string(),
            serde_json::json!({ "promptQueueing": true }),
        );

        Ok(InitializeResponse::new(ProtocolVersion::LATEST)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .session_capabilities(session_caps)
                    .prompt_capabilities(
                        PromptCapabilities::new()
                            .image(true)
                            .embedded_context(true),
                    )
                    .mcp_capabilities(McpCapabilities::new().http(true).sse(true))
                    .meta(meta),
            )
            .agent_info(Implementation::new("trogon-acp", "0.1.0")))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        let session_id = uuid::Uuid::new_v4().to_string();
        info!(session_id = %session_id, cwd = ?args.cwd, "New ACP session");

        let cwd = args.cwd.to_string_lossy().to_string();
        let state = SessionState {
            cwd,
            created_at: now_iso8601(),
            mode: "default".to_string(),
            ..Default::default()
        };
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id = %session_id, error = %e, "Failed to initialise session KV");
        }

        let sid = SessionId::from(session_id.clone());
        self.publish_session_ready(&session_id).await;
        self.send_available_commands_update(&sid).await;

        let modes = Self::build_mode_state(&state.mode);
        let models = Self::build_model_state(&self.default_model);
        let config_options = Self::build_config_options(&state.mode, &self.default_model);

        Ok(NewSessionResponse::new(sid)
            .modes(modes)
            .models(models)
            .config_options(config_options))
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        let session_id = args.session_id.to_string();
        info!(session_id = %session_id, "Load ACP session");

        let state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load session: {e}"),
            )
        })?;

        self.replay_history(&args.session_id, &state).await;
        self.publish_session_ready(&session_id).await;
        self.send_available_commands_update(&args.session_id).await;

        let current_mode = if state.mode.is_empty() { "default" } else { &state.mode };
        let current_model = state.model.as_deref().unwrap_or(&self.default_model);

        let modes = Self::build_mode_state(current_mode);
        let models = Self::build_model_state(current_model);
        let config_options = Self::build_config_options(current_mode, current_model);

        Ok(LoadSessionResponse::new()
            .modes(modes)
            .models(models)
            .config_options(config_options))
    }

    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        let session_id = args.session_id.to_string();
        let mode_id = args.mode_id.to_string();
        info!(session_id = %session_id, mode = %mode_id, "Set session mode");

        let mut state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(ErrorCode::InternalError.into(), format!("Failed to load session: {e}"))
        })?;
        state.mode = mode_id.clone();
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id, error = %e, "Failed to save session mode");
        }

        let current_model = state.model.as_deref().unwrap_or(&self.default_model);

        // Notify client of mode change
        let mode_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(mode_id.clone())),
        );
        let _ = self.notification_sender.send(mode_notification).await;

        // Send updated config options
        let config_options = Self::build_config_options(&mode_id, current_model);
        let config_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options)),
        );
        let _ = self.notification_sender.send(config_notification).await;

        Ok(SetSessionModeResponse::new())
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        let session_id = args.session_id.to_string();
        let config_id = args.config_id.0.as_ref();
        let value = args.value.0.to_string();

        let mut state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(ErrorCode::InternalError.into(), format!("Failed to load session: {e}"))
        })?;

        if config_id == "mode" {
            state.mode = value.clone();
            if let Err(e) = self.store.save(&session_id, &state).await {
                warn!(session_id, error = %e, "Failed to save session mode");
            }
            let notification = SessionNotification::new(
                args.session_id.clone(),
                SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(value.clone())),
            );
            let _ = self.notification_sender.send(notification).await;
        } else if config_id == "model" {
            state.model = Some(value.clone());
            if let Err(e) = self.store.save(&session_id, &state).await {
                warn!(session_id, error = %e, "Failed to save session model");
            }
        }

        let current_mode = if state.mode.is_empty() { "default" } else { &state.mode };
        let current_model = state.model.as_deref().unwrap_or(&self.default_model);
        let config_options = Self::build_config_options(current_mode, current_model);

        let config_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options.clone())),
        );
        let _ = self.notification_sender.send(config_notification).await;

        Ok(SetSessionConfigOptionResponse::new(config_options))
    }

    async fn set_session_model(
        &self,
        args: SetSessionModelRequest,
    ) -> Result<SetSessionModelResponse> {
        let session_id = args.session_id.to_string();
        let model = args.model_id.0.to_string();
        info!(session_id = %session_id, model = %model, "Set session model");

        let mut state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(ErrorCode::InternalError.into(), format!("Failed to load session: {e}"))
        })?;
        state.model = Some(model.clone());
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id, error = %e, "Failed to save session model");
        }

        let current_mode = if state.mode.is_empty() { "default" } else { &state.mode };
        let config_options = Self::build_config_options(current_mode, &model);
        let config_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options)),
        );
        let _ = self.notification_sender.send(config_notification).await;

        Ok(SetSessionModelResponse::new())
    }

    async fn list_sessions(&self, _args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        let ids = self.store.list_ids().await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to list sessions: {e}"),
            )
        })?;

        let mut sessions = Vec::with_capacity(ids.len());
        for id in &ids {
            let state = self.store.load(id).await.unwrap_or_default();
            let cwd = PathBuf::from(if state.cwd.is_empty() { "/" } else { &state.cwd });
            let mut info = SessionInfo::new(id.clone(), cwd);
            if !state.created_at.is_empty() {
                info = info.updated_at(state.created_at.clone());
            }
            if !state.title.is_empty() {
                info = info.title(state.title.clone());
            }
            sessions.push(info);
        }
        Ok(ListSessionsResponse::new(sessions))
    }

    async fn fork_session(&self, args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        let src_id = args.session_id.to_string();
        info!(src_session_id = %src_id, "Fork ACP session");

        let src_state = self.store.load(&src_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load source session: {e}"),
            )
        })?;

        let new_id = uuid::Uuid::new_v4().to_string();
        let cwd = args.cwd.to_string_lossy().to_string();
        let new_state = SessionState {
            messages: src_state.messages.clone(),
            model: src_state.model.clone(),
            mode: src_state.mode.clone(),
            cwd,
            created_at: now_iso8601(),
            title: src_state.title.clone(),
        };
        if let Err(e) = self.store.save(&new_id, &new_state).await {
            warn!(session_id = %new_id, error = %e, "Failed to save forked session");
        }

        let sid = SessionId::from(new_id.clone());
        self.publish_session_ready(&new_id).await;
        self.send_available_commands_update(&sid).await;

        let current_mode = if new_state.mode.is_empty() { "default" } else { &new_state.mode };
        let current_model = new_state.model.as_deref().unwrap_or(&self.default_model);

        Ok(ForkSessionResponse::new(sid)
            .modes(Self::build_mode_state(current_mode))
            .models(Self::build_model_state(current_model))
            .config_options(Self::build_config_options(current_mode, current_model)))
    }

    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        let session_id = args.session_id.to_string();
        info!(session_id = %session_id, "Resume ACP session");

        let state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(ErrorCode::InternalError.into(), format!("Failed to load session: {e}"))
        })?;

        self.publish_session_ready(&session_id).await;
        self.send_available_commands_update(&args.session_id).await;

        let current_mode = if state.mode.is_empty() { "default" } else { &state.mode };
        let current_model = state.model.as_deref().unwrap_or(&self.default_model);

        Ok(ResumeSessionResponse::new()
            .modes(Self::build_mode_state(current_mode))
            .models(Self::build_model_state(current_model))
            .config_options(Self::build_config_options(current_mode, current_model)))
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        agent_client_protocol::Agent::prompt(&self.bridge, args).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        agent_client_protocol::Agent::cancel(&self.bridge, args).await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        // Handle session/close — not yet in agent-client-protocol 0.9.5
        if args.method.as_ref().contains("close") {
            let params: serde_json::Value =
                serde_json::from_str(args.params.get()).unwrap_or_default();
            if let Some(sid) = params.get("sessionId").and_then(|v| v.as_str()) {
                info!(session_id = %sid, "Close ACP session (ext_method)");
                self.close_session_impl(sid).await;
            }
            return Ok(ExtResponse::new(
                serde_json::value::RawValue::NULL.to_owned().into(),
            ));
        }
        Err(Error::new(
            ErrorCode::MethodNotFound.into(),
            format!("unknown ext method: {}", args.method),
        ))
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Ok(())
    }
}

/// Returns the current UTC time as an ISO-8601 string.
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            let (y, mo, day, h, min, s) = epoch_to_parts(secs);
            format!("{y:04}-{mo:02}-{day:02}T{h:02}:{min:02}:{s:02}Z")
        })
        .unwrap_or_default()
}

fn epoch_to_parts(mut secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    secs /= 60;
    let min = secs % 60;
    secs /= 60;
    let h = secs % 24;
    secs /= 24;
    let mut days = secs;
    let mut year = 1970u64;
    loop {
        let dy = days_in_year(year);
        if days < dy { break; }
        days -= dy;
        year += 1;
    }
    let mut month = 1u64;
    loop {
        let dm = days_in_month(year, month);
        if days < dm { break; }
        days -= dm;
        month += 1;
    }
    (year, month, days + 1, h, min, s)
}

fn is_leap(y: u64) -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 }
fn days_in_year(y: u64) -> u64 { if is_leap(y) { 366 } else { 365 } }
fn days_in_month(y: u64, m: u64) -> u64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap(y) { 29 } else { 28 },
        _ => 30,
    }
}
