/// Core NATS publish (fire-and-forget notification). Stream: GLOBAL_EXT (observability).
#[derive(Debug)]
pub struct ExtNotifySubject {
    prefix: crate::acp_prefix::AcpPrefix,
    method: String,
}

impl ExtNotifySubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, method: &str) -> Self {
        Self {
            prefix: prefix.clone(),
            method: method.to_string(),
        }
    }
}

impl std::fmt::Display for ExtNotifySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.ext.{}", self.prefix.as_str(), self.method)
    }
}

impl super::super::markers::Publishable for ExtNotifySubject {}
