use std::fmt;

use crate::agent_id::AgentId;
use crate::node_id::NodeId;

/// What a binding attaches to: one Agent, or one hierarchy node so that every
/// Agent placed at or below it is covered (ADR#0071 Decision 1).
///
/// A subject is an exact identity. There is no pattern arm and there never
/// will be: a relabel must not move authority onto a different Agent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    /// One Agent.
    Agent(AgentId),
    /// One hierarchy node.
    Node(NodeId),
}

impl From<AgentId> for Subject {
    fn from(value: AgentId) -> Self {
        Self::Agent(value)
    }
}

impl From<NodeId> for Subject {
    fn from(value: NodeId) -> Self {
        Self::Node(value)
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent(id) => write!(f, "agent {id}"),
            Self::Node(id) => write!(f, "node {id}"),
        }
    }
}
