#[derive(Debug, Clone)]
pub struct ElicitationCompletedSubject {
    prefix: crate::McpPrefix,
    client_id: crate::McpPeerId,
}

impl ElicitationCompletedSubject {
    pub fn new(prefix: &crate::McpPrefix, client_id: &crate::McpPeerId) -> Self {
        Self {
            prefix: prefix.clone(),
            client_id: client_id.clone(),
        }
    }
}

impl std::fmt::Display for ElicitationCompletedSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.client.{}.notifications.elicitation.complete",
            self.prefix.as_str(),
            self.client_id.as_str()
        )
    }
}

impl super::super::markers::Publishable for ElicitationCompletedSubject {}
