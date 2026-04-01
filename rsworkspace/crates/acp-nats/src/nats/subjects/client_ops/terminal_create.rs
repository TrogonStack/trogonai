/// Agent -> bridge. Core NATS request/reply.
#[derive(Debug)]
pub struct TerminalCreateSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl TerminalCreateSubject {
    pub fn new(
        prefix: &crate::acp_prefix::AcpPrefix,
        session_id: &crate::session_id::AcpSessionId,
    ) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
        }
    }
}

impl std::fmt::Display for TerminalCreateSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.client.terminal.create",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl super::super::markers::ClientRequestable for TerminalCreateSubject {}

impl super::super::stream::StreamAssignment for TerminalCreateSubject {
    const STREAM: Option<super::super::stream::AcpStream> =
        Some(super::super::stream::AcpStream::ClientOps);
}
