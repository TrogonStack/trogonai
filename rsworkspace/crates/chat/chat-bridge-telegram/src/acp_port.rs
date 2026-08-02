use acp_nats::AgentHandler;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::{
    CancelNotification, CloseSessionRequest, ContentBlock, InitializeResponse, NewSessionRequest, PromptRequest,
    StopReason, TextContent,
};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};
use trogon_channel::{
    AgentPort, AgentPortError, AgentSessionId, ConversationRecord, InboundEvent, PromptOutcome, ReleaseReason,
    ReleaseStep, SessionRelease,
};

pub type AcpBridge =
    acp_nats::Bridge<async_nats::Client, trogon_std::time::SystemClock, trogon_nats::jetstream::NatsJetStreamClient>;

#[derive(Debug, thiserror::Error)]
pub enum AcpPortError {
    #[error("agent request failed: {0}")]
    Rpc(agent_client_protocol::Error),
}

impl AgentPortError for AcpPortError {
    fn is_session_lost(&self) -> bool {
        // The acp-nats bridge maps every transport failure and timeout to
        // `InternalError` and passes agent-returned errors through untouched,
        // so only the codes an agent uses to reject an unknown session id mean
        // the session is actually gone.
        match self {
            Self::Rpc(error) => matches!(error.code, ErrorCode::InvalidParams | ErrorCode::ResourceNotFound),
        }
    }
}

/// A session lifecycle method whose availability an agent declares at
/// initialize. ACP requires the client to check before calling one, so nothing
/// in this port reaches for a lifecycle method without asking
/// [`SessionMethods`] first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMethod {
    Load,
    List,
    Delete,
    Resume,
    Close,
    AdditionalDirectories,
}

impl SessionMethod {
    const ALL: [Self; 6] = [
        Self::Load,
        Self::List,
        Self::Delete,
        Self::Resume,
        Self::Close,
        Self::AdditionalDirectories,
    ];

    fn wire_name(self) -> &'static str {
        match self {
            Self::Load => "session/load",
            Self::List => "session/list",
            Self::Delete => "session/delete",
            Self::Resume => "session/resume",
            Self::Close => "session/close",
            Self::AdditionalDirectories => "additionalDirectories",
        }
    }
}

/// What the agent said it can do with sessions, captured once from the
/// `initialize` response. Every capability in ACP is present-means-supported,
/// which is easy to invert by accident; reading them through one value object
/// keeps that decision in a single place.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionMethods {
    load: bool,
    list: bool,
    delete: bool,
    resume: bool,
    close: bool,
    additional_directories: bool,
}

impl SessionMethods {
    #[must_use]
    pub fn advertised(response: &InitializeResponse) -> Self {
        let sessions = &response.agent_capabilities.session_capabilities;
        Self {
            load: response.agent_capabilities.load_session,
            list: sessions.list.is_some(),
            delete: sessions.delete.is_some(),
            resume: sessions.resume.is_some(),
            close: sessions.close.is_some(),
            additional_directories: sessions.additional_directories.is_some(),
        }
    }

    #[must_use]
    pub fn supports(self, method: SessionMethod) -> bool {
        match method {
            SessionMethod::Load => self.load,
            SessionMethod::List => self.list,
            SessionMethod::Delete => self.delete,
            SessionMethod::Resume => self.resume,
            SessionMethod::Close => self.close,
            SessionMethod::AdditionalDirectories => self.additional_directories,
        }
    }
}

impl std::fmt::Display for SessionMethods {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for method in SessionMethod::ALL {
            if !self.supports(method) {
                continue;
            }
            if !first {
                f.write_str(", ")?;
            }
            f.write_str(method.wire_name())?;
            first = false;
        }
        if first {
            f.write_str("none")?;
        }
        Ok(())
    }
}

/// The ACP implementation of [`AgentPort`]: forwards session/prompt calls
/// through the acp-nats Bridge. Streamed agent output does not come back
/// through this port; it arrives at the bridge's ACP client half
/// (`TelegramRenderClient`) as session notifications.
pub struct AcpPort {
    bridge: Arc<AcpBridge>,
    agent_cwd: PathBuf,
    methods: SessionMethods,
}

impl AcpPort {
    pub fn new(bridge: Arc<AcpBridge>, agent_cwd: PathBuf, methods: SessionMethods) -> Self {
        Self {
            bridge,
            agent_cwd,
            methods,
        }
    }
}

/// Human-readable context prefix: the only part of the conversational
/// metadata a non-participating agent is guaranteed to see, since only prompt
/// text reaches the model.
fn prompt_text(event: &InboundEvent) -> String {
    let body = event.text.as_deref().unwrap_or_default();
    format!("[telegram message from {}]\n{}", event.sender.display_name, body)
}

/// Structured twin of the context prefix, for agents that opt into reading
/// `_meta` (see the architecture doc's `_meta` convention).
fn prompt_meta(event: &InboundEvent) -> agent_client_protocol::schema::v1::Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "chat".to_string(),
        serde_json::json!({
            "channel": event.endpoint.channel(),
            "endpoint": event.endpoint.kv_key(),
            "sender": {
                "platform_user_id": event.sender.platform_user_id,
                "display_name": event.sender.display_name,
            },
            "message_ref": event.message_ref,
            "occurred_at": event.occurred_at,
        }),
    );
    meta
}

impl AgentPort for AcpPort {
    type Error = AcpPortError;

    async fn create_session(&self, _conversation: &ConversationRecord) -> Result<AgentSessionId, Self::Error> {
        let response = self
            .bridge
            .new_session(NewSessionRequest::new(self.agent_cwd.clone()))
            .await
            .map_err(AcpPortError::Rpc)?;
        Ok(AgentSessionId::new(response.session_id.to_string()))
    }

    async fn prompt(&self, session: &AgentSessionId, event: &InboundEvent) -> Result<PromptOutcome, Self::Error> {
        let mut request = PromptRequest::new(
            session.as_str().to_string(),
            vec![ContentBlock::Text(TextContent::new(prompt_text(event)))],
        );
        request.meta = Some(prompt_meta(event));

        let response = self.bridge.prompt(request).await.map_err(AcpPortError::Rpc)?;
        Ok(match response.stop_reason {
            StopReason::EndTurn => PromptOutcome::Completed,
            StopReason::Cancelled => PromptOutcome::Cancelled,
            StopReason::Refusal => PromptOutcome::Refused,
            _ => PromptOutcome::Truncated,
        })
    }

    async fn cancel(&self, session: &AgentSessionId) -> Result<(), Self::Error> {
        self.bridge
            .cancel(CancelNotification::new(session.as_str().to_string()))
            .await
            .map_err(AcpPortError::Rpc)
    }

    /// Stop the turn first, then hand the session back. Cancel before close so
    /// an agent mid-turn is told to wind down rather than having the session
    /// pulled from under it; both steps are bounded by the bridge's operation
    /// timeout and neither can fail the release. Deleting is deliberately not
    /// part of this: the bridge is done with the session, which is not the same
    /// as the user asking for its history to be destroyed.
    async fn release_session(&self, session: &AgentSessionId, reason: ReleaseReason) -> SessionRelease {
        let cancelled = match self.cancel(session).await {
            Ok(()) => ReleaseStep::Done,
            Err(error) => {
                warn!(session = %session, reason = ?reason, error = %error, "Cancel failed while releasing session");
                ReleaseStep::Failed
            }
        };

        let closed = if self.methods.supports(SessionMethod::Close) {
            match self
                .bridge
                .close_session(CloseSessionRequest::new(session.as_str().to_string()))
                .await
            {
                Ok(_) => ReleaseStep::Done,
                Err(error) => {
                    warn!(session = %session, reason = ?reason, error = %error, "Close failed while releasing session");
                    ReleaseStep::Failed
                }
            }
        } else {
            info!(session = %session, "Agent does not advertise session/close; releasing without it");
            ReleaseStep::Unsupported
        };

        SessionRelease { cancelled, closed }
    }
}
