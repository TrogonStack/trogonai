/// Agent -> bridge. Core NATS request/reply. Stream: CLIENT_OPS.
#[derive(Debug)]
pub struct TerminalWaitForExitSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl TerminalWaitForExitSubject {
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

impl std::fmt::Display for TerminalWaitForExitSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.client.terminal.wait_for_exit",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl super::super::markers::ClientRequestable for TerminalWaitForExitSubject {}
