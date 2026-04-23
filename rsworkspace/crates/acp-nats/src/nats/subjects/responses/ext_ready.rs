/// Agent -> bridge signal.
#[derive(Debug)]
pub struct ExtReadySubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl ExtReadySubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, session_id: &crate::session_id::AcpSessionId) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
        }
    }
}

impl std::fmt::Display for ExtReadySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.agent.ext.ready",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl super::super::markers::Publishable for ExtReadySubject {}

impl super::super::stream::StreamAssignment for ExtReadySubject {
    const STREAM: Option<super::super::stream::AcpStream> = Some(super::super::stream::AcpStream::Responses);
}
