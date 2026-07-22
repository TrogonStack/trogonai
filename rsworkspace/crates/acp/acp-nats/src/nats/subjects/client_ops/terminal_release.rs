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

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> crate::acp_prefix::AcpPrefix {
        crate::acp_prefix::AcpPrefix::new("acp").expect("prefix")
    }
    fn session_id() -> crate::session_id::AcpSessionId {
        crate::session_id::AcpSessionId::new("ses1").expect("session_id")
    }

    #[test]
    fn display_formats_subject_correctly() {
        let s = TerminalReleaseSubject::new(&prefix(), &session_id());
        assert_eq!(s.to_string(), "acp.session.ses1.client.terminal.release");
    }
}
