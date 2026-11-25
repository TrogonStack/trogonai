use crate::agent_id::AgentId;
use crate::callee::Callee;
use crate::node_id::NodeId;

/// One thing a delegation permission names: an exact Agent, or every Agent
/// placed at or below a hierarchy node (ADR#0071 Decision 6).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DelegationTarget {
    /// One Agent, by exact identity.
    Agent(AgentId),
    /// Every Agent placed at or below the node.
    AgentsUnder(NodeId),
}

impl DelegationTarget {
    /// Returns whether the resolved callee is named exactly or is placed at or
    /// below the node.
    pub fn covers(&self, callee: &Callee) -> bool {
        match self {
            Self::Agent(agent) => agent == callee.agent(),
            Self::AgentsUnder(node) => callee.is_placed_under(node),
        }
    }
}
