use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `agent/getAuthenticatedExtendedCard` — fetch agent card after authentication.
#[derive(Debug)]
pub struct AgentCardSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl AgentCardSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for AgentCardSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.agent.{}.agent.card",
            self.prefix.as_str(),
            self.agent_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for AgentCardSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Requestable for AgentCardSubject {}

impl super::super::stream::StreamAssignment for AgentCardSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}
