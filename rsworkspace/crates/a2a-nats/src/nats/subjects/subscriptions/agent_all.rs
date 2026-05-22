use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `{prefix}.agent.{agent_id}.>` — agent-side subscription for all methods routed to
/// this agent. The agent binary subscribes here and dispatches to handlers per subject.
#[derive(Debug)]
pub struct AgentAllSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl AgentAllSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for AgentAllSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.{}.>", self.prefix.as_str(), self.agent_id.as_str())
    }
}

impl async_nats::subject::ToSubject for AgentAllSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for AgentAllSubject {}

impl super::super::stream::StreamAssignment for AgentAllSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}
