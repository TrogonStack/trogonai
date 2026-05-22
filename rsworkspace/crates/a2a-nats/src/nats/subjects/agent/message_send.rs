use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `message/send` — unary JSON-RPC method delivering a single message and reading a response.
#[derive(Debug)]
pub struct MessageSendSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl MessageSendSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for MessageSendSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.agent.{}.message.send",
            self.prefix.as_str(),
            self.agent_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for MessageSendSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Requestable for MessageSendSubject {}

impl super::super::stream::StreamAssignment for MessageSendSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}
