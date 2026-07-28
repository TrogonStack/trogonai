use crate::conversation::ConversationRecord;
use crate::event::InboundChatEvent;
use serde::{Deserialize, Serialize};

/// An agent-side session handle. Opaque to everything except the port
/// implementation that minted it: only meaningful at the agent it belongs to,
/// which is why a conversation stores it next to (never instead of) the
/// agent binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentSessionId(String);

impl AgentSessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a prompt turn ended, protocol-neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutcome {
    Completed,
    Cancelled,
    Refused,
    /// The turn ended for a protocol-specific reason the bridge treats as
    /// completed-with-caveats (e.g. token or turn limits).
    Truncated,
}

/// The one seam between chat routing and agent protocols. One implementation
/// per protocol (ACP first; A2A and HTTP later); the implementation owns all
/// protocol specifics including how streamed agent output reaches the
/// renderer. Deliberately not a NATS namespace: protocol-neutral agent
/// addressability already exists per protocol (see the architecture doc).
#[allow(async_fn_in_trait)]
pub trait AgentPort {
    type Error: std::error::Error + 'static;

    /// Create a fresh session for a conversation on its bound agent.
    async fn create_session(&self, conversation: &ConversationRecord) -> Result<AgentSessionId, Self::Error>;

    /// Send one inbound event as a prompt and wait for the turn to end.
    /// Streamed output is delivered out-of-band by the implementation.
    async fn prompt(&self, session: &AgentSessionId, event: &InboundChatEvent) -> Result<PromptOutcome, Self::Error>;

    async fn cancel(&self, session: &AgentSessionId) -> Result<(), Self::Error>;
}
