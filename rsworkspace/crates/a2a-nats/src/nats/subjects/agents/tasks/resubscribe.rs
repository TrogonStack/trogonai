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
            "{}.agents.{}.tasks.resubscribe",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_prefix_agent_tasks_resubscribe_subject() {
        let s = TasksResubscribeSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
        assert_eq!(s.to_string(), "a2a.agents.planner.tasks.resubscribe");
    }

    #[test]
    fn to_subject_round_trips_display_form() {
        use async_nats::subject::ToSubject;
        let s = TasksResubscribeSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
        assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.tasks.resubscribe");
    }
}
