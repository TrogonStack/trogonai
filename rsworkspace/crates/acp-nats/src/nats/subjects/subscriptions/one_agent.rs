/// Wildcard: all agent subjects for one session.
#[derive(Debug)]
pub struct OneAgentSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl OneAgentSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, session_id: &crate::session_id::AcpSessionId) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
        }
    }
}

impl std::fmt::Display for OneAgentSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.agent.>",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for OneAgentSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for OneAgentSubject {}

impl super::super::stream::StreamAssignment for OneAgentSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}
