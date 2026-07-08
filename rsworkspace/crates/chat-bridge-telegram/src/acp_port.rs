use agent_client_protocol::{
    Agent, CancelNotification, ContentBlock, NewSessionRequest, PromptRequest, StopReason, TextContent,
};
use std::path::PathBuf;
use std::rc::Rc;
use trogon_chat::{AgentPort, AgentSessionId, ConversationRecord, InboundChatEvent, PromptOutcome};

pub type AcpBridge =
    acp_nats::Bridge<async_nats::Client, trogon_std::time::SystemClock, trogon_nats::jetstream::NatsJetStreamClient>;

#[derive(Debug, thiserror::Error)]
pub enum AcpPortError {
    #[error("agent request failed: {0}")]
    Rpc(agent_client_protocol::Error),
}

/// The ACP implementation of [`AgentPort`]: forwards session/prompt calls
/// through the acp-nats Bridge. Streamed agent output does not come back
/// through this port; it arrives at the bridge's ACP client half
/// (`TelegramRenderClient`) as session notifications.
pub struct AcpPort {
    bridge: Rc<AcpBridge>,
    agent_cwd: PathBuf,
}

impl AcpPort {
    pub fn new(bridge: Rc<AcpBridge>, agent_cwd: PathBuf) -> Self {
        Self { bridge, agent_cwd }
    }
}

/// Human-readable context prefix: the only part of the conversational
/// metadata a non-participating agent is guaranteed to see, since only prompt
/// text reaches the model.
fn prompt_text(event: &InboundChatEvent) -> String {
    let body = event.text.as_deref().unwrap_or_default();
    format!(
        "[telegram message from {}]\n{}",
        event.sender.display_name, body
    )
}

/// Structured twin of the context prefix, for agents that opt into reading
/// `_meta` (see the architecture doc's `_meta` convention).
fn prompt_meta(event: &InboundChatEvent) -> agent_client_protocol::Meta {
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

    async fn prompt(&self, session: &AgentSessionId, event: &InboundChatEvent) -> Result<PromptOutcome, Self::Error> {
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
}
