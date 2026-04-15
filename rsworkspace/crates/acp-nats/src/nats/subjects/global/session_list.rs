/// Core NATS request/reply.
#[derive(Debug)]
pub struct SessionListSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl SessionListSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for SessionListSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.session.list", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for SessionListSubject {}

impl super::super::stream::StreamAssignment for SessionListSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> crate::acp_prefix::AcpPrefix {
        crate::acp_prefix::AcpPrefix::new("acp").expect("prefix")
    }

    #[test]
    fn display_formats_subject_correctly() {
        let s = SessionListSubject::new(&prefix());
        assert_eq!(s.to_string(), "acp.agent.session.list");
    }
}
