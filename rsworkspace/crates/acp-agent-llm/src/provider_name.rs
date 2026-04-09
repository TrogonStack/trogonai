#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderName(String);

#[derive(Debug, Clone, PartialEq)]
pub struct UnknownProvider(pub String);

impl std::fmt::Display for UnknownProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown provider: {}", self.0)
    }
}

impl std::error::Error for UnknownProvider {}

impl ProviderName {
    pub fn new(s: impl Into<String>) -> Result<Self, UnknownProvider> {
        let s = s.into();
        match s.as_str() {
            "anthropic" | "openai" => Ok(Self(s)),
            _ => Err(UnknownProvider(s)),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProviderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_providers() {
        assert!(ProviderName::new("anthropic").is_ok());
        assert!(ProviderName::new("openai").is_ok());
    }

    #[test]
    fn rejects_unknown_provider() {
        let err = ProviderName::new("unknown").unwrap_err();
        assert_eq!(err, UnknownProvider("unknown".to_string()));
    }

    #[test]
    fn as_str_returns_value() {
        let name = ProviderName::new("anthropic").unwrap();
        assert_eq!(name.as_str(), "anthropic");
    }
}
