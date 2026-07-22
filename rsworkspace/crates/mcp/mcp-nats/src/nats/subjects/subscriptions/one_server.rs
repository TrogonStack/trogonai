#[derive(Debug, Clone)]
pub struct OneServerSubject {
    prefix: crate::McpPrefix,
    server_id: crate::McpPeerId,
}

impl OneServerSubject {
    pub fn new(prefix: &crate::McpPrefix, server_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            server_id: server_id.clone(),
        }
    }
}

impl std::fmt::Display for OneServerSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.server.{}.>", self.prefix.as_str(), self.server_id.as_str())
    }
}

impl super::super::markers::Subscribable for OneServerSubject {}
