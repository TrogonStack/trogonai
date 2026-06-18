use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `tasks/pushNotificationConfig/set` — register a webhook for task lifecycle events.
#[derive(Debug)]
pub struct PushSetSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl PushSetSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for PushSetSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agents.{}.push.set", self.prefix.as_str(), self.agent_id.as_str())
    }
}

impl async_nats::subject::ToSubject for PushSetSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::super::markers::Requestable for PushSetSubject {}

impl super::super::super::stream::StreamAssignment for PushSetSubject {
    const STREAM: Option<super::super::super::stream::A2aStream> = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_prefix_agent_push_set_subject() {
        let s = PushSetSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
        assert_eq!(s.to_string(), "a2a.agents.planner.push.set");
    }

    #[test]
    fn to_subject_round_trips_display_form() {
        use async_nats::subject::ToSubject;
        let s = PushSetSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
        assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.push.set");
    }
}
