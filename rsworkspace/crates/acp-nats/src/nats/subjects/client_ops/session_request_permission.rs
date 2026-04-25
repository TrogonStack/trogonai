/// Agent -> bridge. Core NATS request/reply.
#[derive(Debug)]
pub struct SessionRequestPermissionSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl SessionRequestPermissionSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, session_id: &crate::session_id::AcpSessionId) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
        }
    }
}

impl std::fmt::Display for SessionRequestPermissionSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.client.session.request_permission",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl super::super::markers::ClientRequestable for SessionRequestPermissionSubject {}

impl super::super::stream::StreamAssignment for SessionRequestPermissionSubject {
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
        let s = SessionRequestPermissionSubject::new(&prefix(), &session_id());
        assert_eq!(
            s.to_string(),
            "acp.session.ses1.client.session.request_permission"
        );
    }
}
