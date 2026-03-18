//! `TrogonAcpAgent` — local implementation of the ACP [`Agent`] trait.
//!
//! Handles `initialize`, `authenticate`, `new_session`, `set_session_mode`, and
//! `load_session` in-process (no NATS round-trip).  Delegates `prompt` and `cancel`
//! to the inner [`Bridge`], which routes them through NATS to the Runner.

use std::time::Duration;

use agent_client_protocol::{
    AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification, ContentBlock,
    ContentChunk, Error, ErrorCode, ExtNotification, ExtRequest, ExtResponse,
    Implementation, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ProtocolVersion, Result, SessionId, SessionNotification, SessionUpdate,
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
        let body = serde_json::to_vec(
            &serde_json::json!({ "sessionId": session_id }),
        )
        .unwrap_or_default();

        tokio::spawn(async move {
            tokio::time::sleep(SESSION_READY_DELAY).await;
            if let Err(e) = nats.publish(subject.clone(), body.into()).await {
                warn!(subject = %subject, error = %e, "Failed to publish session.ready");
            }
        });
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

        Ok(InitializeResponse::new(ProtocolVersion::LATEST)
            .agent_capabilities(AgentCapabilities::new().load_session(true))
            .agent_info(Implementation::new("trogon-acp", "0.1.0")))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        let session_id = uuid::Uuid::new_v4().to_string();
        info!(session_id = %session_id, cwd = ?args.cwd, "New ACP session");

        if let Err(e) = self.store.save(&session_id, &SessionState::default()).await {
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

        // Replay prior assistant turns to the client as AgentMessageChunk notifications.
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
                        args.session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            ContentBlock::Text(TextContent::new(text)),
                        )),
                    );
                    if self.notification_sender.send(notification).await.is_err() {
                        warn!("notification receiver dropped during load_session history replay");
                        break;
                    }
                }
            }
        }

        self.publish_session_ready(&session_id).await;

        Ok(LoadSessionResponse::new())
    }

    async fn set_session_mode(
        &self,
        _args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        Ok(SetSessionModeResponse::new())
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
        Err(Error::new(ErrorCode::InternalError.into(), "not implemented"))
    }
}
