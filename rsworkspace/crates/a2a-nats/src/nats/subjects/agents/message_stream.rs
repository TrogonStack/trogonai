use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `message/stream` — bootstrap subject. The agent responds with the task envelope
/// (containing `task_id`) and then publishes events to `{prefix}.tasks.{task_id}.events.{req_id}`.
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
            "{}.agents.{}.message.stream",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_prefix_agent_message_stream_subject() {
        let s = MessageStreamSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
        assert_eq!(s.to_string(), "a2a.agents.planner.message.stream");
    }

    #[test]
    fn to_subject_round_trips_display_form() {
        use async_nats::subject::ToSubject;
        let s = MessageStreamSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
        assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.message.stream");
    }
}
