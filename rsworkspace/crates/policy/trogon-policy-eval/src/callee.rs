use crate::agent_id::AgentId;
use crate::chain::Chain;
use crate::node_id::NodeId;
use crate::subject::Subject;

/// The single Agent a delegation has already resolved to, and the hierarchy
/// nodes it is placed under.
///
/// ADR#0071 Decision 6 resolves the selector first and checks the permission
/// second, on the one resolved Agent. The evaluator never resolves a selector,
/// so this is what the caller hands it after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callee {
    agent: AgentId,
    chain: Chain,
}

impl Callee {
    /// Names the resolved Agent and the ancestor nodes it sits under.
    pub fn new(agent: AgentId, ancestors: impl IntoIterator<Item = NodeId>) -> Self {
        let chain = Chain::starting_with(Subject::Agent(agent.clone()), ancestors.into_iter().map(Subject::Node));
        Self { agent, chain }
    }

    /// The resolved Agent.
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// The Agent followed by its ancestor nodes.
    pub fn chain(&self) -> &Chain {
        &self.chain
    }

    /// Returns whether the Agent is placed at or below this node.
    pub fn is_placed_under(&self, node: &NodeId) -> bool {
        self.chain
            .iter()
            .any(|subject| matches!(subject, Subject::Node(candidate) if candidate == node))
    }
}
