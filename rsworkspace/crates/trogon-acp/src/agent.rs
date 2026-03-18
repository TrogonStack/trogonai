//! `TrogonAcpAgent` — local implementation of the ACP [`Agent`] trait.
//!
//! Handles `initialize`, `authenticate`, `new_session`, `set_session_mode`,
//! `load_session`, `resume_session`, `fork_session`, `list_sessions`, and
//! `set_session_model` in-process (no NATS round-trip).  Delegates `prompt`
//! and `cancel` to the inner [`Bridge`], which routes them through NATS to
//! the Runner.

use std::path::PathBuf;
use std::time::Duration;

use agent_client_protocol::{
    AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification, ContentBlock,
    ContentChunk, Error, ErrorCode, ExtNotification, ExtRequest, ExtResponse, ForkSessionRequest,
    ForkSessionResponse, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, ProtocolVersion, Result,
    ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionForkCapabilities,
    SessionId, SessionInfo, SessionListCapabilities, SessionNotification,
    SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModelRequest, SetSessionModelResponse,
    SetSessionModeRequest, SetSessionModeResponse, TextContent,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

use acp_nats::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use acp_nats::Bridge;
use trogon_acp_runner::{SessionState, SessionStore};
use trogon_agent::agent_loop::ContentBlock as AgentContentBlock;
use trogon_std::time::GetElapsed;

const SESSION_READY_DELAY: Duration = Duration::from_millis(100);

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
    ) -> Self {
        Self {
            bridge,
            store,
            nats,
            prefix: prefix.into(),
            notification_sender,
        }
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

    /// Replay assistant message history to the client as `AgentMessageChunk` notifications.
    async fn replay_history(&self, session_id: &SessionId, state: &SessionState) {
        for msg in &state.messages {
            if msg.role == "assistant" {
                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let AgentContentBlock::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                if !text.is_empty() {
                    let notification = SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            ContentBlock::Text(TextContent::new(text)),
                        )),
                    );
                    if self.notification_sender.send(notification).await.is_err() {
                        warn!("notification receiver dropped during history replay");
                        break;
                    }
                }
            }
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

        let session_caps = SessionCapabilities::new()
            .list(SessionListCapabilities::new())
            .fork(SessionForkCapabilities::new())
            .resume(SessionResumeCapabilities::new());

        Ok(InitializeResponse::new(ProtocolVersion::LATEST)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .session_capabilities(session_caps),
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
            created_at: chrono_now(),
            ..Default::default()
        };
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id = %session_id, error = %e, "Failed to initialise session KV");
        }

        self.publish_session_ready(&session_id).await;

        Ok(NewSessionResponse::new(SessionId::from(session_id)))
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

        Ok(LoadSessionResponse::new())
    }

    async fn set_session_mode(
        &self,
        _args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        Ok(SetSessionModeResponse::new())
    }

    async fn set_session_config_option(
        &self,
        _args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        Ok(SetSessionConfigOptionResponse::new(vec![]))
    }

    async fn set_session_model(
        &self,
        args: SetSessionModelRequest,
    ) -> Result<SetSessionModelResponse> {
        let session_id = args.session_id.to_string();
        let model = args.model_id.0.to_string();
        info!(session_id = %session_id, model = %model, "Set session model");

        let mut state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load session: {e}"),
            )
        })?;

        state.model = Some(model);
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id = %session_id, error = %e, "Failed to save session model");
        }

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
                info = info.updated_at(state.created_at);
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
            cwd,
            created_at: chrono_now(),
        };

        if let Err(e) = self.store.save(&new_id, &new_state).await {
            warn!(session_id = %new_id, error = %e, "Failed to save forked session");
        }

        self.publish_session_ready(&new_id).await;

        Ok(ForkSessionResponse::new(SessionId::from(new_id)))
    }

    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        let session_id = args.session_id.to_string();
        info!(session_id = %session_id, "Resume ACP session");

        // Verify session exists (creates empty if not)
        let _ = self.store.load(&session_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load session: {e}"),
            )
        })?;

        // resume_session does NOT replay history — just signals the session is ready
        self.publish_session_ready(&session_id).await;

        Ok(ResumeSessionResponse::new())
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        agent_client_protocol::Agent::prompt(&self.bridge, args).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        agent_client_protocol::Agent::cancel(&self.bridge, args).await
    }

    async fn ext_method(&self, _args: ExtRequest) -> Result<ExtResponse> {
        Err(Error::new(ErrorCode::InternalError.into(), "not implemented"))
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Ok(())
    }
}

/// Returns the current UTC time as an ISO-8601 string (best-effort; falls back to empty string).
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            // Format as ISO-8601 without pulling in chrono
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
    // Days since 1970-01-01
    let mut days = secs;
    let mut year = 1970u64;
    loop {
        let dy = days_in_year(year);
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let mut month = 1u64;
    loop {
        let dm = days_in_month(year, month);
        if days < dm {
            break;
        }
        days -= dm;
        month += 1;
    }
    (year, month, days + 1, h, min, s)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_year(y: u64) -> u64 {
    if is_leap(y) { 366 } else { 365 }
}

fn days_in_month(y: u64, m: u64) -> u64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap(y) { 29 } else { 28 },
        _ => 30,
    }
}
