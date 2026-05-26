use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookUrl(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookUrlError {
    raw: String,
}

impl WebhookUrl {
    pub fn new(raw: impl Into<String>) -> Result<Self, WebhookUrlError> {
        let raw = raw.into();
        if raw.starts_with("https://") || raw.starts_with("http://") {
            Ok(Self(raw))
        } else {
            Err(WebhookUrlError { raw })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WebhookUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for WebhookUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "webhook URL must start with https:// or http://: {}", self.raw)
    }
}

impl std::error::Error for WebhookUrlError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https() {
        assert!(WebhookUrl::new("https://example.com/hook").is_ok());
    }

    #[test]
    fn accepts_http_for_local_testing() {
        assert!(WebhookUrl::new("http://localhost:8080/hook").is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(WebhookUrl::new("ftp://example.com/hook").is_err());
        assert!(WebhookUrl::new("nats://example.com").is_err());
        assert!(WebhookUrl::new("subject:a2a.push.t.caller.task").is_err());
        assert!(WebhookUrl::new("").is_err());
    }

    #[test]
    fn display_roundtrips_url() {
        let url = WebhookUrl::new("https://example.com/hook").unwrap();
        assert_eq!(url.to_string(), "https://example.com/hook");
    }

    #[test]
    fn error_display_includes_raw_value() {
        let err = WebhookUrl::new("ftp://bad").unwrap_err();
        assert!(err.to_string().contains("ftp://bad"));
    }
}
