/// Close session.
#[derive(Debug)]
pub struct CloseSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl CloseSubject {
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

impl std::fmt::Display for CloseSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.agent.close",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for CloseSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::SessionCommand for CloseSubject {}

impl super::super::stream::StreamAssignment for CloseSubject {
    const STREAM: Option<super::super::stream::AcpStream> =
        Some(super::super::stream::AcpStream::Commands);
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::subject::ToSubject as _;

    fn prefix() -> crate::acp_prefix::AcpPrefix {
        crate::acp_prefix::AcpPrefix::new("acp").expect("prefix")
    }
    fn session_id() -> crate::session_id::AcpSessionId {
        crate::session_id::AcpSessionId::new("ses1").expect("session_id")
    }

    #[test]
    fn display_formats_subject_correctly() {
        let s = CloseSubject::new(&prefix(), &session_id());
        assert_eq!(s.to_string(), "acp.session.ses1.agent.close");
    }

    #[test]
    fn to_subject_matches_display() {
        let s = CloseSubject::new(&prefix(), &session_id());
        assert_eq!(s.to_subject().as_str(), s.to_string());
    }
}
