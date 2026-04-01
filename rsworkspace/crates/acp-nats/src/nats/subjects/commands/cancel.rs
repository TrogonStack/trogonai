/// Cancel in-flight prompt.
#[derive(Debug)]
pub struct CancelSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl CancelSubject {
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

impl std::fmt::Display for CancelSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.agent.cancel",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for CancelSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Publishable for CancelSubject {}

impl super::super::stream::StreamAssignment for CancelSubject {
    const STREAM: Option<super::super::stream::AcpStream> =
        Some(super::super::stream::AcpStream::Commands);
}
