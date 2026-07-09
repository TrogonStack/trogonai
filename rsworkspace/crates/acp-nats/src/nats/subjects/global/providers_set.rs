/// Core NATS request/reply.
#[derive(Debug)]
pub struct ProvidersSetSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl ProvidersSetSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for ProvidersSetSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.providers.set", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for ProvidersSetSubject {}

impl super::super::stream::StreamAssignment for ProvidersSetSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}
