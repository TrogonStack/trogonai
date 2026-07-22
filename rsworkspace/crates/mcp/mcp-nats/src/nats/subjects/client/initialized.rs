#[derive(Debug, Clone)]
pub struct InitializedSubject {
    prefix: crate::McpPrefix,
    server_id: crate::McpPeerId,
}

impl InitializedSubject {
    pub fn new(prefix: &crate::McpPrefix, server_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            server_id: server_id.clone(),
        }
    }
}

impl std::fmt::Display for InitializedSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.server.{}.notifications.initialized",
            self.prefix.as_str(),
            self.server_id.as_str()
        )
    }
}

impl super::super::markers::Publishable for InitializedSubject {}
