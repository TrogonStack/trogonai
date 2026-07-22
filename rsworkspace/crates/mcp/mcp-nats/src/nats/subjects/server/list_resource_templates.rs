#[derive(Debug, Clone)]
pub struct ListResourceTemplatesSubject {
    prefix: crate::McpPrefix,
    server_id: crate::McpPeerId,
}

impl ListResourceTemplatesSubject {
    pub fn new(prefix: &crate::McpPrefix, server_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            server_id: server_id.clone(),
        }
    }
}

impl std::fmt::Display for ListResourceTemplatesSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.server.{}.resources.templates.list",
            self.prefix.as_str(),
            self.server_id.as_str()
        )
    }
}

impl super::super::markers::Requestable for ListResourceTemplatesSubject {}
