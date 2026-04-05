use std::fmt;
use std::sync::Arc;

#[derive(Clone)]
pub struct WebhookSecret(Arc<str>);

#[derive(Debug, PartialEq, Eq)]
pub struct EmptyWebhookSecret;

impl fmt::Display for EmptyWebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("webhook secret must not be empty")
    }
}

impl std::error::Error for EmptyWebhookSecret {}

impl WebhookSecret {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptyWebhookSecret> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(EmptyWebhookSecret);
        }
        Ok(Self(Arc::from(s)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WebhookSecret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_secret() {
        let secret = WebhookSecret::new("my-secret").unwrap();
        assert_eq!(secret.as_str(), "my-secret");
    }

    #[test]
    fn empty_secret_rejected() {
        assert!(matches!(WebhookSecret::new(""), Err(EmptyWebhookSecret)));
    }

    #[test]
    fn debug_redacts_value() {
        let secret = WebhookSecret::new("super-secret").unwrap();
        assert_eq!(format!("{secret:?}"), "WebhookSecret(***)");
    }

    #[test]
    fn clone_shares_arc() {
        let a = WebhookSecret::new("secret").unwrap();
        let b = a.clone();
        assert_eq!(a.as_str(), b.as_str());
    }

    #[test]
    fn error_display() {
        assert_eq!(
            EmptyWebhookSecret.to_string(),
            "webhook secret must not be empty"
        );
    }
}
