/// Wildcard: all session agent subjects.
#[derive(Debug)]
pub struct AllAgentSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl AllAgentSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for AllAgentSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.session.*.agent.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for AllAgentSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for AllAgentSubject {}

impl super::super::stream::StreamAssignment for AllAgentSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::subject::ToSubject as _;

    fn prefix() -> crate::acp_prefix::AcpPrefix {
        crate::acp_prefix::AcpPrefix::new("acp").expect("prefix")
    }

    #[test]
    fn display_formats_subject_correctly() {
        let s = AllAgentSubject::new(&prefix());
        assert_eq!(s.to_string(), "acp.session.*.agent.>");
    }

    #[test]
    fn to_subject_matches_display() {
        let s = AllAgentSubject::new(&prefix());
        assert_eq!(s.to_subject().as_str(), s.to_string());
    }
}
