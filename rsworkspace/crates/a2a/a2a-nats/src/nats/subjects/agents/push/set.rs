use trogon_identity_types::SubjectPrincipal;

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

/// `tasks/pushNotificationConfig/set` — register a webhook for task lifecycle events.
#[derive(Debug)]
pub struct PushSetSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
    caller: SubjectPrincipal,
}

impl PushSetSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId, caller: &SubjectPrincipal) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
            caller: caller.clone(),
        }
    }
}

impl std::fmt::Display for PushSetSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.agents.{}.{}.push.set",
            self.prefix.as_str(),
            self.agent_id.as_str(),
            self.caller
        )
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
mod tests;
