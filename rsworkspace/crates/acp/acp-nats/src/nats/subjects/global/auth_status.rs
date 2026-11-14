#[derive(Debug)]
pub struct AuthStatusSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl AuthStatusSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for AuthStatusSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.global.agent.auth.status", self.prefix.as_str())
    }
}

impl super::super::markers::Requestable for AuthStatusSubject {}

impl super::super::stream::StreamAssignment for AuthStatusSubject {
    const STREAM: Option<super::super::stream::AcpStream> = None;
}
