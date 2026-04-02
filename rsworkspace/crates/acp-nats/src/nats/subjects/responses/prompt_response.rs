/// Agent -> bridge streamed response.
#[derive(Debug)]
pub struct PromptResponseSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
    req_id: crate::req_id::ReqId,
}

impl PromptResponseSubject {
    pub fn new(
        prefix: &crate::acp_prefix::AcpPrefix,
        session_id: &crate::session_id::AcpSessionId,
        req_id: &crate::req_id::ReqId,
    ) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
            req_id: req_id.clone(),
        }
    }
}

impl std::fmt::Display for PromptResponseSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.agent.prompt.response.{}",
            self.prefix.as_str(),
            self.session_id.as_str(),
            self.req_id
        )
    }
}

impl async_nats::subject::ToSubject for PromptResponseSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for PromptResponseSubject {}

impl super::super::stream::StreamAssignment for PromptResponseSubject {
    const STREAM: Option<super::super::stream::AcpStream> =
        Some(super::super::stream::AcpStream::Responses);
}
