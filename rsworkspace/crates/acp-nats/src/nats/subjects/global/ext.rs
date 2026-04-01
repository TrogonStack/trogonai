/// Core NATS request/reply.
#[derive(Debug)]
pub struct ExtSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    method: String,
}

impl ExtSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, method: &str) -> Self {
        Self {
            prefix: prefix.clone(),
            method: method.to_string(),
        }
    }
}

impl std::fmt::Display for ExtSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.ext.{}", self.prefix.as_str(), self.method)
    }
}

impl super::super::markers::Requestable for ExtSubject {}

impl super::super::stream::StreamAssignment for ExtSubject {
    const STREAM: Option<super::super::stream::AcpStream> =
        Some(super::super::stream::AcpStream::GlobalExt);
}
