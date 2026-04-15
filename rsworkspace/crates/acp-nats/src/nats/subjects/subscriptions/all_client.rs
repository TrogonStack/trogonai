/// Wildcard: all session client subjects.
#[derive(Debug)]
pub struct AllClientSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl AllClientSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for AllClientSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.session.*.client.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for AllClientSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for AllClientSubject {}

impl super::super::stream::StreamAssignment for AllClientSubject {
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
        let s = AllClientSubject::new(&prefix());
        assert_eq!(s.to_string(), "acp.session.*.client.>");
    }

    #[test]
    fn to_subject_matches_display() {
        let s = AllClientSubject::new(&prefix());
        assert_eq!(s.to_subject().as_str(), s.to_string());
    }
}
