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

    /// Standard capability record for an **Explore** agent.
    ///
    /// Explore agents read files and answer questions — they never edit,
    /// execute shell commands, or perform destructive operations.
    pub fn explore(nats_subject: impl Into<String>) -> Self {
        Self::new(
            "Explore",
            [
                "file_search",
                "code_lookup",
                "read_only",
                "question_answering",
            ],
            nats_subject,
        )
    }

    /// Standard capability record for a **Plan** agent.
    ///
    /// Plan agents analyse requirements and produce implementation plans —
    /// they never execute destructive tools or write production code directly.
    pub fn plan(nats_subject: impl Into<String>) -> Self {
        Self::new(
            "Plan",
            [
                "architecture",
                "implementation_planning",
                "trade_off_analysis",
                "read_only",
            ],
            nats_subject,
        )
    }

    /// True if the agent advertises `capability` (case-insensitive).
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities
            .iter()
            .any(|c| c.eq_ignore_ascii_case(capability))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(caps: &[&str]) -> AgentCapability {
        AgentCapability::new("TestActor", caps.iter().copied(), "actors.test.>")
    }

    #[test]
    fn new_sets_defaults() {
        let a = agent(&["code_review"]);
        assert_eq!(a.agent_type, "TestActor");
        assert_eq!(a.nats_subject, "actors.test.>");
        assert_eq!(a.current_load, 0);
        assert_eq!(a.metadata, serde_json::Value::Null);
    }

    #[test]
    fn has_capability_exact_match() {
        let a = agent(&["code_review", "security"]);
        assert!(a.has_capability("code_review"));
    }

    #[test]
    fn has_capability_case_insensitive() {
        let a = agent(&["CodeReview"]);
        assert!(a.has_capability("codereview"));
        assert!(a.has_capability("CODEREVIEW"));
        assert!(a.has_capability("CodeReview"));
    }

    #[test]
    fn has_capability_absent_returns_false() {
        let a = agent(&["code_review"]);
        assert!(!a.has_capability("security_analysis"));
    }

    #[test]
    fn has_capability_empty_capabilities_returns_false() {
        let a = agent(&[]);
        assert!(!a.has_capability("anything"));
    }

    #[test]
    fn serde_round_trip() {
        let original = agent(&["review", "deploy"]);
        let json = serde_json::to_string(&original).unwrap();
        let restored: AgentCapability = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn explore_agent_type_and_capabilities() {
        let a = AgentCapability::explore("agents.explore.>");
        assert_eq!(a.agent_type, "Explore");
        assert_eq!(a.nats_subject, "agents.explore.>");
        assert!(a.has_capability("read_only"));
        assert!(a.has_capability("file_search"));
        assert!(a.has_capability("code_lookup"));
    }

    #[test]
    fn plan_agent_type_and_capabilities() {
        let a = AgentCapability::plan("agents.plan.>");
        assert_eq!(a.agent_type, "Plan");
        assert_eq!(a.nats_subject, "agents.plan.>");
        assert!(a.has_capability("read_only"));
        assert!(a.has_capability("architecture"));
        assert!(a.has_capability("implementation_planning"));
    }

    #[test]
    fn explore_does_not_have_destructive_capabilities() {
        let a = AgentCapability::explore("agents.explore.>");
        assert!(!a.has_capability("execute_shell"));
        assert!(!a.has_capability("write_file"));
    }

    #[test]
    fn plan_does_not_have_destructive_capabilities() {
        let a = AgentCapability::plan("agents.plan.>");
        assert!(!a.has_capability("execute_shell"));
        assert!(!a.has_capability("write_file"));
    }
}
