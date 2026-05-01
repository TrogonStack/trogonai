#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId(String);

#[derive(Debug, Clone, PartialEq)]
pub struct EmptyModelId;

impl std::fmt::Display for EmptyModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("model id cannot be empty")
    }
}

impl std::error::Error for EmptyModelId {}

impl ModelId {
    pub fn new(s: impl Into<String>) -> Result<Self, EmptyModelId> {
        let s = s.into();
        if s.is_empty() {
            return Err(EmptyModelId);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_non_empty() {
        let id = ModelId::new("claude-opus-4-6").unwrap();
        assert_eq!(id.as_str(), "claude-opus-4-6");
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(ModelId::new("").unwrap_err(), EmptyModelId);
    }

    #[test]
    fn display_returns_value() {
        let id = ModelId::new("gpt-4o").unwrap();
        assert_eq!(format!("{id}"), "gpt-4o");
    }
}
