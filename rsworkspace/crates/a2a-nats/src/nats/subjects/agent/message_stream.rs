use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `message/stream` — bootstrap subject. The agent responds with the task envelope
/// (containing `task_id`) and then publishes events to `{prefix}.task.{task_id}.events.{req_id}`.
#[derive(Debug)]
pub struct MessageStreamSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl MessageStreamSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for MessageStreamSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.agent.{}.message.stream",
            self.prefix.as_str(),
            self.agent_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for MessageStreamSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Requestable for MessageStreamSubject {}

impl super::super::stream::StreamAssignment for MessageStreamSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}
