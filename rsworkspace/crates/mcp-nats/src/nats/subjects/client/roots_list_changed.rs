#[derive(Debug, Clone)]
pub struct RootsListChangedSubject {
    prefix: crate::McpPrefix,
    server_id: crate::McpPeerId,
}

impl RootsListChangedSubject {
    pub fn new(prefix: &crate::McpPrefix, server_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            server_id: server_id.clone(),
        }
    }
}

impl std::fmt::Display for RootsListChangedSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.server.{}.notifications.roots.list_changed",
            self.prefix.as_str(),
            self.server_id.as_str()
        )
    }
}

impl super::super::markers::Publishable for RootsListChangedSubject {}
