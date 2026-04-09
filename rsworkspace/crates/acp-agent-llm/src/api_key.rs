use trogon_std::secret_string::{EmptySecret, SecretString};

#[derive(Clone)]
pub struct ApiKey(SecretString);

impl ApiKey {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecret> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(****)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_non_empty() {
        let key = ApiKey::new("sk-test-123").unwrap();
        assert_eq!(key.as_str(), "sk-test-123");
    }

    #[test]
    fn rejects_empty() {
        assert!(ApiKey::new("").is_err());
    }

    #[test]
    fn debug_redacts() {
        let key = ApiKey::new("sk-secret").unwrap();
        assert_eq!(format!("{key:?}"), "ApiKey(****)");
    }
}
