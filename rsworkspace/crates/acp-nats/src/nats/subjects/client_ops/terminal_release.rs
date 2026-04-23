/// Agent -> bridge. Core NATS request/reply.
#[derive(Debug)]
pub struct TerminalReleaseSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl TerminalReleaseSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, session_id: &crate::session_id::AcpSessionId) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
        }
    }
}

impl std::fmt::Display for TerminalReleaseSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.client.terminal.release",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl super::super::markers::ClientRequestable for TerminalReleaseSubject {}

impl super::super::stream::StreamAssignment for TerminalReleaseSubject {
    const STREAM: Option<super::super::stream::AcpStream> = Some(super::super::stream::AcpStream::ClientOps);
}
