#[cfg(test)]
#[path = "agent_port_tests.rs"]
mod agent_port_tests;

use crate::conversation::ConversationRecord;
use crate::endpoint::EndpointError;
use crate::event::InboundEvent;
use crate::safe_token::SafeToken;
use serde::{Deserialize, Deserializer, Serialize};

/// An agent-side session handle. Opaque to everything except the port
/// implementation that minted it: only meaningful at the agent it belongs to,
/// which is why a conversation stores it next to (never instead of) the
/// agent binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentSessionId(SafeToken);

impl AgentSessionId {
    pub fn new(id: impl Into<String>) -> Result<Self, EndpointError> {
        Ok(Self(SafeToken::new(id)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for AgentSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentSessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
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

/// Port errors carry the one distinction the routing layer must act on. Every
/// port classifies its own protocol's failures; nothing above this trait
/// inspects error codes.
pub trait AgentPortError: std::error::Error + 'static {
    /// True when the agent may no longer have the session, which a fresh
    /// session repairs. Transport failures, timeouts, and agent-internal
    /// errors are false: rotating on those would discard a live conversation
    /// that was merely unreachable for a moment.
    ///
    /// A hint, not a verdict. A protocol need not carry a distinct "no such
    /// session" code, so whatever code rejects an unknown session id may also
    /// reject a prompt the agent simply dislikes. A caller must therefore never
    /// destroy conversation state on this alone: it may open a fresh session
    /// and keep it only once that session has actually answered.
    fn is_session_lost(&self) -> bool;
}

/// Why a session is being released. Carried into port logs and available to
/// protocols that can pass a reason to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    /// The user asked for a fresh conversation.
    NewSession,
    /// A session opened to repair a suspected lost session is being handed back
    /// unused: either it failed the same way the old one did, or it answered and
    /// the conversation could not be pointed at it, which leaves its reply
    /// unreadable either way.
    RepairFailed,
    /// A suspected lost session was replaced by a fresh one that answered. The
    /// suspicion is a guess, so the agent may still hold the old session; it is
    /// told to let go rather than left holding one nothing points at.
    Replaced,
}

/// How one step of the release ladder ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseStep {
    Done,
    /// The agent does not advertise the capability, so nothing was sent.
    Unsupported,
    /// The step was attempted and failed. Recorded, never fatal.
    Failed,
}

/// Report of a best-effort release. The conversation does not point at the
/// session by the time this runs, whether it dropped the pointer or never took
/// one, so no step here can fail anything; the report exists so an operator can
/// see what the agent did with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionRelease {
    pub cancelled: ReleaseStep,
    pub closed: ReleaseStep,
}

/// The one seam between channel routing and agent protocols. One implementation
/// per protocol (ACP first; A2A and HTTP later); the implementation owns all
/// protocol specifics including how streamed agent output reaches the
/// renderer. Deliberately not a NATS namespace: protocol-neutral agent
/// addressability already exists per protocol (see the architecture doc).
#[allow(async_fn_in_trait)]
pub trait AgentPort {
    type Error: AgentPortError;

    /// Create a fresh session for a conversation on its bound agent.
    async fn create_session(&self, conversation: &ConversationRecord) -> Result<AgentSessionId, Self::Error>;

    /// Send one inbound event as a prompt and wait for the turn to end.
    /// Streamed output is delivered out-of-band by the implementation.
    async fn prompt(&self, session: &AgentSessionId, event: &InboundEvent) -> Result<PromptOutcome, Self::Error>;

    async fn cancel(&self, session: &AgentSessionId) -> Result<(), Self::Error>;

    /// Tell the agent the bridge is done with a session so it can stop work
    /// and free resources. Infallible on purpose: an agent that cannot or will
    /// not release must never wedge the conversation it was released from.
    async fn release_session(&self, session: &AgentSessionId, reason: ReleaseReason) -> SessionRelease;
}
