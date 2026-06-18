#[derive(Debug, Clone)]
pub struct ResourceListChangedSubject {
    prefix: crate::McpPrefix,
    client_id: crate::McpPeerId,
}

impl ResourceListChangedSubject {
    pub fn new(prefix: &crate::McpPrefix, client_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            client_id: client_id.clone(),
        }
    }
}

impl std::fmt::Display for ResourceListChangedSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.client.{}.notifications.resources.list_changed",
            self.prefix.as_str(),
            self.client_id.as_str()
        )
    }
}

impl super::super::markers::Publishable for ResourceListChangedSubject {}
