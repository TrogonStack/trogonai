use crate::agent_port::AgentSessionId;
use crate::endpoint::PrincipalId;
use serde::{Deserialize, Serialize};

/// Which configured agent a conversation is bound to. Resolution from id to
/// protocol + address is bridge/router configuration, never stored here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(String);

impl ConversationId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub fn from_string(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConversationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The durable half of a conversation. The agent binding is sticky (set once
/// by routing policy at creation, changed only by explicit rebind); the
/// session is ephemeral and belongs to the agent, replaced freely without
/// re-running policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub principal: PrincipalId,
    pub agent_id: AgentId,
    pub current_session: Option<AgentSessionId>,
    /// Unix seconds; supplied by the caller (this crate takes no clock).
    pub created_at: i64,
    pub last_activity_at: i64,
}
