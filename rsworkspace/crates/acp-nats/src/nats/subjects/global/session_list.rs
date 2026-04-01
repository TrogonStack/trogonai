/// Core NATS request/reply. No stream.
#[derive(Debug)]
pub struct SessionListSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl SessionListSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for SessionListSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.session.list", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for SessionListSubject {}
