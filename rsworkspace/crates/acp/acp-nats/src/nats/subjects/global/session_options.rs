/// Core NATS request/reply. Global because `sessionId` is optional on `session/options`.
#[derive(Debug)]
pub struct SessionOptionsSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl SessionOptionsSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for SessionOptionsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.global.agent.session.options", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for SessionOptionsSubject {}

impl super::super::stream::StreamAssignment for SessionOptionsSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}
