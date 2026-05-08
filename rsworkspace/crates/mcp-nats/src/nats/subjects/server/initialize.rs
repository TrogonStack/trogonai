#[derive(Debug, Clone)]
pub struct InitializeSubject {
    prefix: crate::McpPrefix,
    server_id: crate::McpPeerId,
}

impl InitializeSubject {
    pub fn new(prefix: &crate::McpPrefix, server_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            server_id: server_id.clone(),
        }
    }
}

impl std::fmt::Display for InitializeSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.server.{}.initialize",
            self.prefix.as_str(),
            self.server_id.as_str()
        )
    }
}

impl super::super::markers::Requestable for InitializeSubject {}
