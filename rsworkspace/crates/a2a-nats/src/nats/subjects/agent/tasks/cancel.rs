use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `tasks/cancel` — request cancellation of a running task.
#[derive(Debug)]
pub struct TasksCancelSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl TasksCancelSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for TasksCancelSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.agent.{}.tasks.cancel",
            self.prefix.as_str(),
            self.agent_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for TasksCancelSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::super::markers::Requestable for TasksCancelSubject {}

impl super::super::super::stream::StreamAssignment for TasksCancelSubject {
    const STREAM: Option<super::super::super::stream::A2aStream> = None;
}
