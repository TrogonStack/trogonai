/// Core NATS request/reply. Stream: GLOBAL (observability).
#[derive(Debug)]
pub struct InitializeSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl InitializeSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for InitializeSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.initialize", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for InitializeSubject {}
