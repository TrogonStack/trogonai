/// Fork session.
#[derive(Debug)]
pub struct ForkSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl ForkSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, session_id: &crate::session_id::AcpSessionId) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
        }
    }
}

impl std::fmt::Display for ForkSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.agent.fork",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for ForkSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::SessionCommand for ForkSubject {}

impl super::super::stream::StreamAssignment for ForkSubject {
    const STREAM: Option<super::super::stream::AcpStream> = Some(super::super::stream::AcpStream::Commands);
}
