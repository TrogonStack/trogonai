/// Wildcard: all prompt subjects across sessions.
#[derive(Debug)]
pub struct PromptWildcardSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl PromptWildcardSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for PromptWildcardSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.session.*.agent.prompt", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for PromptWildcardSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for PromptWildcardSubject {}

impl super::super::stream::StreamAssignment for PromptWildcardSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}
