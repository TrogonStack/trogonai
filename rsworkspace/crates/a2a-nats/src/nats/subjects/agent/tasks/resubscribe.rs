use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `tasks/resubscribe` — request a JetStream consumer that replays task events from
/// a client-supplied last seen sequence (carried in the `X-Resubscribe-Start-Seq` header).
#[derive(Debug)]
pub struct TasksResubscribeSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl TasksResubscribeSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for TasksResubscribeSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.agent.{}.tasks.resubscribe",
            self.prefix.as_str(),
            self.agent_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for TasksResubscribeSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::super::markers::Requestable for TasksResubscribeSubject {}

impl super::super::super::stream::StreamAssignment for TasksResubscribeSubject {
    const STREAM: Option<super::super::super::stream::A2aStream> = None;
}
