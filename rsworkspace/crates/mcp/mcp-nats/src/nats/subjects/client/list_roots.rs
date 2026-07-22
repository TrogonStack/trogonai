#[derive(Debug, Clone)]
pub struct ListRootsSubject {
    prefix: crate::McpPrefix,
    client_id: crate::McpPeerId,
}

impl ListRootsSubject {
    pub fn new(prefix: &crate::McpPrefix, client_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            client_id: client_id.clone(),
        }
    }
}

impl std::fmt::Display for ListRootsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.client.{}.roots.list",
            self.prefix.as_str(),
            self.client_id.as_str()
        )
    }
}

impl super::super::markers::Requestable for ListRootsSubject {}
