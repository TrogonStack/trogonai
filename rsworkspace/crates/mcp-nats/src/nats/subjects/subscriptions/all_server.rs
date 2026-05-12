#[derive(Debug, Clone)]
pub struct AllServerSubject {
    prefix: crate::McpPrefix,
}

impl AllServerSubject {
    pub fn new(prefix: &crate::McpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for AllServerSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.server.>", self.prefix.as_str())
    }
}

impl super::super::markers::Subscribable for AllServerSubject {}
