use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `tasks/get` — fetch the current state of a task.
#[derive(Debug)]
pub struct TasksGetSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl TasksGetSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for TasksGetSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.{}.tasks.get", self.prefix.as_str(), self.agent_id.as_str())
    }
}

impl async_nats::subject::ToSubject for TasksGetSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::super::markers::Requestable for TasksGetSubject {}

impl super::super::super::stream::StreamAssignment for TasksGetSubject {
    const STREAM: Option<super::super::super::stream::A2aStream> = None;
}
