#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AcpConnectionId(uuid::Uuid);

impl AcpConnectionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for AcpConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AcpConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_generates_non_empty_id() {
        assert!(!AcpConnectionId::new().to_string().is_empty());
    }
}
