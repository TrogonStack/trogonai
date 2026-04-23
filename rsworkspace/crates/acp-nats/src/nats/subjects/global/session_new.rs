/// Core NATS request/reply.
#[derive(Debug)]
pub struct SessionNewSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl SessionNewSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for SessionNewSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.session.new", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for SessionNewSubject {}

impl super::super::stream::StreamAssignment for SessionNewSubject {
    const STREAM: Option<super::super::stream::AcpStream> = Some(super::super::stream::AcpStream::Global);
}
