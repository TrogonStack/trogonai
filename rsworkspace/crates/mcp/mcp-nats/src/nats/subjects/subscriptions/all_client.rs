#[derive(Debug, Clone)]
pub struct AllClientSubject {
    prefix: crate::McpPrefix,
}

impl AllClientSubject {
    pub fn new(prefix: &crate::McpPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for AllClientSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.client.>", self.prefix.as_str())
    }
}

impl super::super::markers::Subscribable for AllClientSubject {}
