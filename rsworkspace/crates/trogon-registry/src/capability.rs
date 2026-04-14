use serde::{Deserialize, Serialize};

/// Everything the Router needs to know about one registered agent.
///
/// Agents publish this record into the `AGENT_REGISTRY` KV bucket at startup
/// and refresh it every [`crate::provision::HEARTBEAT_INTERVAL`] seconds.
/// Entries that are not refreshed expire after [`crate::provision::ENTRY_TTL`]
/// and are automatically removed from the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCapability {
    /// Identifies the agent type, e.g. `"PrActor"`, `"IncidentActor"`.
    /// Also used as the KV key — one registration per agent type.
    pub agent_type: String,

    /// Human-readable tags describing what this agent can do.
    /// Used by the Router when presenting options to the LLM.
    /// Example: `["code_review", "security_analysis"]`
    pub capabilities: Vec<String>,

    /// The NATS subject pattern this agent listens on for incoming events.
    /// Example: `"actors.pr.>"`, `"actors.incident.>"`
    pub nats_subject: String,

    /// Current number of in-flight event handlers.
    /// Refreshed with every heartbeat so the Router can make load-aware decisions.
    pub current_load: u32,

    /// Arbitrary agent-specific metadata (version, region, feature flags, …).
    pub metadata: serde_json::Value,
}

impl AgentCapability {
    pub fn new(
        agent_type: impl Into<String>,
        capabilities: impl IntoIterator<Item = impl Into<String>>,
        nats_subject: impl Into<String>,
    ) -> Self {
        Self {
            agent_type: agent_type.into(),
            capabilities: capabilities.into_iter().map(Into::into).collect(),
            nats_subject: nats_subject.into(),
            current_load: 0,
            metadata: serde_json::Value::Null,
        }
    }

    /// True if the agent advertises `capability` (case-insensitive).
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities
            .iter()
            .any(|c| c.eq_ignore_ascii_case(capability))
    }
}
