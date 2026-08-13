/// Agent -> bridge async notification.
///
/// Scoped to the session, not the request (ADR#0055). Requests within a session
/// are told apart by the JSON-RPC `id`, projected to `Jsonrpc-Id`, which the
/// bridge mints per request.
#[derive(Debug)]
pub struct UpdateSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl UpdateSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, session_id: &crate::session_id::AcpSessionId) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
        }
    }
}

impl std::fmt::Display for UpdateSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.session.{}.agent.update",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for UpdateSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for UpdateSubject {}

impl super::super::stream::StreamAssignment for UpdateSubject {
    const STREAM: Option<super::super::stream::AcpStream> = Some(super::super::stream::AcpStream::Notifications);
}

#[cfg(test)]
mod tests;
