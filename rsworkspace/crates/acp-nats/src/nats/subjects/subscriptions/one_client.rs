/// Wildcard: all client subjects for one session.
#[derive(Debug)]
pub struct OneClientSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
}

impl OneClientSubject {
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

impl std::fmt::Display for OneClientSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.session.{}.client.>",
            self.prefix.as_str(),
            self.session_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for OneClientSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for OneClientSubject {}

impl super::super::stream::StreamAssignment for OneClientSubject {
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
        let s = OneClientSubject::new(&prefix(), &session_id());
        assert_eq!(s.to_string(), "acp.session.ses1.client.>");
    }

    #[test]
    fn to_subject_matches_display() {
        let s = OneClientSubject::new(&prefix(), &session_id());
        assert_eq!(s.to_subject().as_str(), s.to_string());
    }
}
