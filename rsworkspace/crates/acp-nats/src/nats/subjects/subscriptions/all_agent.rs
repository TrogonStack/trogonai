/// Wildcard: all session agent subjects.
#[derive(Debug)]
pub struct AllAgentSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl AllAgentSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for AllAgentSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.session.*.agent.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for AllAgentSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for AllAgentSubject {}

impl super::super::stream::StreamAssignment for AllAgentSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}
