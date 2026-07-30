#[derive(Debug, Clone)]
pub struct SubscriptionsListenSubject {
    prefix: crate::McpPrefix,
    server_id: crate::McpPeerId,
}

impl SubscriptionsListenSubject {
    pub fn new(prefix: &crate::McpPrefix, server_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            server_id: server_id.clone(),
        }
    }
}

impl std::fmt::Display for SubscriptionsListenSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.server.{}.subscriptions.listen",
            self.prefix.as_str(),
            self.server_id.as_str()
        )
    }
}

impl super::super::markers::Requestable for SubscriptionsListenSubject {}
