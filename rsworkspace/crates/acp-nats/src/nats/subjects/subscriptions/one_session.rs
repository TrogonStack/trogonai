/// Wildcard: all subjects for one session.
#[derive(Debug)]
pub struct OneSessionSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl OneSessionSubject {
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

impl std::fmt::Display for OneSessionSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.>",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for OneSessionSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for OneSessionSubject {}

impl super::super::stream::StreamAssignment for OneSessionSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
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
        let s = OneSessionSubject::new(&prefix(), &session_id());
        assert_eq!(s.to_string(), "acp.session.ses1.>");
    }

    #[test]
    fn to_subject_matches_display() {
        let s = OneSessionSubject::new(&prefix(), &session_id());
        assert_eq!(s.to_subject().as_str(), s.to_string());
    }
}
