/// Wildcard: all session subjects.
#[derive(Debug)]
pub struct AllSessionSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl AllSessionSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for AllSessionSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.session.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for AllSessionSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for AllSessionSubject {}

impl super::super::stream::StreamAssignment for AllSessionSubject {
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
        let s = AllSessionSubject::new(&prefix());
        assert_eq!(s.to_string(), "acp.session.>");
    }

    #[test]
    fn to_subject_matches_display() {
        let s = AllSessionSubject::new(&prefix());
        assert_eq!(s.to_subject().as_str(), s.to_string());
    }
}
