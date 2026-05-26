use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `tasks/pushNotificationConfig/delete` — remove a push notification configuration.
#[derive(Debug)]
pub struct PushDeleteSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
}

impl PushDeleteSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

impl std::fmt::Display for PushDeleteSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.agent.{}.tasks.push_notification_config.delete",
            self.prefix.as_str(),
            self.agent_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for PushDeleteSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::super::markers::Requestable for PushDeleteSubject {}

impl super::super::super::stream::StreamAssignment for PushDeleteSubject {
    const STREAM: Option<super::super::super::stream::A2aStream> = None;
}
