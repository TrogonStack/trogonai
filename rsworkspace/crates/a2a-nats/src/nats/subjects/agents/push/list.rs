use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `tasks/pushNotificationConfig/list` — list configured push notification webhooks.
#[derive(Debug)]
pub struct PushListSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl PushListSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for PushListSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.agents.{}.push.list",
            self.prefix.as_str(),
            self.agent_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for PushListSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::super::markers::Requestable for PushListSubject {}

impl super::super::super::stream::StreamAssignment for PushListSubject {
    const STREAM: Option<super::super::super::stream::A2aStream> = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_prefix_agent_push_list_subject() {
        let s = PushListSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
        assert_eq!(s.to_string(), "a2a.agents.planner.push.list");
    }

    #[test]
    fn to_subject_round_trips_display_form() {
        use async_nats::subject::ToSubject;
        let s = PushListSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
        assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.push.list");
    }
}
