#[cfg(test)]
#[path = "conversation_tests.rs"]
mod conversation_tests;

use crate::agent_port::AgentSessionId;
use crate::endpoint::{EndpointError, PrincipalId};
use crate::safe_token::SafeToken;
use serde::{Deserialize, Deserializer, Serialize};
use trogon_std::NowV7;

/// Which configured agent a conversation is bound to. Resolution from id to
/// protocol + address is bridge/router configuration, never stored here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentId(SafeToken);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Result<Self, EndpointError> {
        Ok(Self(SafeToken::new(id)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ConversationId(SafeToken);

impl ConversationId {
    /// Opaque and time-ordered: this doubles as the conversation KV key, so
    /// v7 makes the bucket list in creation order. The generator is passed in
    /// for the same reason `ConversationRecord::created_at` is: no ambient
    /// clock in this crate.
    #[allow(clippy::expect_used)]
    pub fn generate(ids: &impl NowV7) -> Self {
        // `simple()` strips the hyphens, leaving only hex digits.
        Self(SafeToken::new(ids.now_v7().simple().to_string()).expect("uuid v7 simple form is a safe token"))
    }

    pub fn from_string(id: impl Into<String>) -> Result<Self, EndpointError> {
        Ok(Self(SafeToken::new(id)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for ConversationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConversationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::from_string(raw).map_err(serde::de::Error::custom)
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
