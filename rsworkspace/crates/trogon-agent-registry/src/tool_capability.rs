//! Resolve a tool call against an [`AgentRecord`]'s allow-list.
//!
//! Resolution ladder (most → least specific):
//! 1. Tool name appears in `agent.allowed_tools` → **Allow**.
//! 2. Tool's declared capability is in `agent.allow_capabilities`
//!    (treated as `Write` when the tool doesn't declare one) → **Allow**.
//! 3. Otherwise → **Deny**.
//!
//! The fail-safe choice — `Write` when unset — matches the
//! ToolCapability decision in `GCP_TODO.md §3.4`.

use crate::types::{AgentRecord, ToolCapability};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityReason {
    ExplicitAllowedTool,
    CapabilityAllowList,
    NotAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityResolution {
    pub decision: CapabilityDecision,
    pub reason: CapabilityReason,
}

/// Decide whether `agent` may call `tool_name` whose tool server
/// declares `tool_capability` (or `None`, treated as `Write`).
pub fn resolve_tool_capability(
    agent: &AgentRecord,
    tool_name: &str,
    tool_capability: Option<ToolCapability>,
) -> CapabilityResolution {
    if agent.allowed_tools.iter().any(|t| t == tool_name) {
        return CapabilityResolution {
            decision: CapabilityDecision::Allow,
            reason: CapabilityReason::ExplicitAllowedTool,
        };
    }
    let cap = tool_capability.unwrap_or(ToolCapability::Write);
    if agent.allow_capabilities.contains(&cap) {
        return CapabilityResolution {
            decision: CapabilityDecision::Allow,
            reason: CapabilityReason::CapabilityAllowList,
        };
    }
    CapabilityResolution {
        decision: CapabilityDecision::Deny,
        reason: CapabilityReason::NotAllowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LifecycleState;

    fn agent(allowed_tools: Vec<&str>, allow_capabilities: Vec<ToolCapability>) -> AgentRecord {
        AgentRecord {
            agent_id: "a".into(),
            agent_version: "1".into(),
            agent_definition_digest: "d".into(),
            owner_team: "t".into(),
            allowed_workloads: vec![],
            allowed_tools: allowed_tools.into_iter().map(String::from).collect(),
            allowed_audiences: vec![],
            allowed_purposes: None,
            mesh_token_ttl_s: None,
            allow_capabilities,
            metadata: serde_json::Value::Null,
            lifecycle_state: LifecycleState::Active,
            created_at: "2026-06-04T00:00:00Z".into(),
            updated_at: "2026-06-04T00:00:00Z".into(),
        }
    }

    #[test]
    fn explicit_tool_beats_capability_check() {
        let a = agent(vec!["set_secret"], vec![ToolCapability::Read]);
        let r = resolve_tool_capability(&a, "set_secret", Some(ToolCapability::Write));
        assert_eq!(r.decision, CapabilityDecision::Allow);
        assert_eq!(r.reason, CapabilityReason::ExplicitAllowedTool);
    }

    #[test]
    fn read_only_agent_allowed_for_read_tool() {
        let a = agent(vec![], vec![ToolCapability::Read]);
        let r = resolve_tool_capability(&a, "get_status", Some(ToolCapability::Read));
        assert_eq!(r.decision, CapabilityDecision::Allow);
        assert_eq!(r.reason, CapabilityReason::CapabilityAllowList);
    }

    #[test]
    fn read_only_agent_denied_for_write_tool() {
        let a = agent(vec![], vec![ToolCapability::Read]);
        let r = resolve_tool_capability(&a, "set_secret", Some(ToolCapability::Write));
        assert_eq!(r.decision, CapabilityDecision::Deny);
        assert_eq!(r.reason, CapabilityReason::NotAllowed);
    }

    #[test]
    fn missing_capability_defaults_to_write_failsafe() {
        let a = agent(vec![], vec![ToolCapability::Read]);
        let r = resolve_tool_capability(&a, "mystery_tool", None);
        assert_eq!(r.decision, CapabilityDecision::Deny);
    }

    #[test]
    fn missing_capability_with_write_allowance_passes() {
        let a = agent(vec![], vec![ToolCapability::Write]);
        let r = resolve_tool_capability(&a, "mystery_tool", None);
        assert_eq!(r.decision, CapabilityDecision::Allow);
    }

    #[test]
    fn empty_lists_deny_by_default() {
        let a = agent(vec![], vec![]);
        let r = resolve_tool_capability(&a, "anything", Some(ToolCapability::Read));
        assert_eq!(r.decision, CapabilityDecision::Deny);
    }
}
