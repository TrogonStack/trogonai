/// Core NATS request/reply.
#[derive(Debug)]
pub struct LogoutSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl LogoutSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for LogoutSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.logout", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for LogoutSubject {}

impl super::super::stream::StreamAssignment for LogoutSubject {
    const STREAM: Option<super::super::stream::AcpStream> =
        Some(super::super::stream::AcpStream::Global);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> crate::acp_prefix::AcpPrefix {
        crate::acp_prefix::AcpPrefix::new("acp").expect("prefix")
    }

    #[test]
    fn display_formats_subject_correctly() {
        let s = LogoutSubject::new(&prefix());
        assert_eq!(s.to_string(), "acp.agent.logout");
    }
}
