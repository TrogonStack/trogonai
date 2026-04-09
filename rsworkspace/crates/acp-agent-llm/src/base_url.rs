#[derive(Debug, Clone)]
pub struct BaseUrl(String);

#[derive(Debug, Clone, PartialEq)]
pub enum BaseUrlError {
    Empty,
    MissingScheme,
}

impl std::fmt::Display for BaseUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("base URL cannot be empty"),
            Self::MissingScheme => f.write_str("base URL must start with http:// or https://"),
        }
    }
}

impl std::error::Error for BaseUrlError {}

impl BaseUrl {
    pub fn new(s: impl Into<String>) -> Result<Self, BaseUrlError> {
        let s = s.into();
        if s.is_empty() {
            return Err(BaseUrlError::Empty);
        }
        if !s.starts_with("http://") && !s.starts_with("https://") {
            return Err(BaseUrlError::MissingScheme);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https() {
        let url = BaseUrl::new("https://api.anthropic.com").unwrap();
        assert_eq!(url.as_str(), "https://api.anthropic.com");
    }

    #[test]
    fn accepts_http() {
        assert!(BaseUrl::new("http://localhost:8080").is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(BaseUrl::new("").unwrap_err(), BaseUrlError::Empty);
    }

    #[test]
    fn rejects_missing_scheme() {
        assert_eq!(
            BaseUrl::new("api.anthropic.com").unwrap_err(),
            BaseUrlError::MissingScheme
        );
    }
}
