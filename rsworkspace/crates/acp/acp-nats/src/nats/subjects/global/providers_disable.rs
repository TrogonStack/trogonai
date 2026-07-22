/// Core NATS request/reply.
#[derive(Debug)]
pub struct ProvidersDisableSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl ProvidersDisableSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for ProvidersDisableSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.providers.disable", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for ProvidersDisableSubject {}

impl super::super::stream::StreamAssignment for ProvidersDisableSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}
