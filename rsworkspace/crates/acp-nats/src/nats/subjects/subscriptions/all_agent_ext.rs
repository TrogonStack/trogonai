/// Wildcard: all session agent extension subjects.
#[derive(Debug)]
pub struct AllAgentExtSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl AllAgentExtSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for AllAgentExtSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.session.*.agent.ext.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for AllAgentExtSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for AllAgentExtSubject {}
