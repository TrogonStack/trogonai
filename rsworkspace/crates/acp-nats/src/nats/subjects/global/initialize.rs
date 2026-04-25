/// Core NATS request/reply.
#[derive(Debug)]
pub struct InitializeSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl InitializeSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for InitializeSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.initialize", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for InitializeSubject {}

impl super::super::stream::StreamAssignment for InitializeSubject {
    const STREAM: Option<super::super::stream::AcpStream> = Some(super::super::stream::AcpStream::Global);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> crate::acp_prefix::AcpPrefix {
        crate::acp_prefix::AcpPrefix::new("acp").expect("prefix")
    }

    #[test]
    fn display_formats_subject_correctly() {
        let s = InitializeSubject::new(&prefix());
        assert_eq!(s.to_string(), "acp.agent.initialize");
    }
}
