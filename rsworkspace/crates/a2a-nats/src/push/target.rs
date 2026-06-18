use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookUrl(url::Url);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookUrlError {
    /// Couldn't parse the input as a URL at all.
    Parse { raw: String, reason: String },
    /// Parsed cleanly but the scheme isn't `http`/`https`.
    UnsupportedScheme { raw: String, scheme: String },
}

impl WebhookUrl {
    pub fn new(raw: impl Into<String>) -> Result<Self, WebhookUrlError> {
        let raw = raw.into();
        let parsed = url::Url::parse(&raw).map_err(|e| WebhookUrlError::Parse {
            raw: raw.clone(),
            reason: e.to_string(),
        })?;
        match parsed.scheme() {
            "http" | "https" => Ok(Self(parsed)),
            other => Err(WebhookUrlError::UnsupportedScheme {
                raw,
                scheme: other.to_owned(),
            }),
        }
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for WebhookUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl fmt::Display for WebhookUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse { raw, reason } => {
                write!(f, "webhook URL is not a valid URL ({reason}): {raw}")
            }
            Self::UnsupportedScheme { raw, scheme } => {
                write!(f, "webhook URL must use http:// or https://, got {scheme}://: {raw}")
            }
        }
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
        let err = WebhookUrl::new("ftp://example.com/hook").unwrap_err();
        assert!(matches!(err, WebhookUrlError::UnsupportedScheme { .. }));
        let err = WebhookUrl::new("nats://example.com").unwrap_err();
        assert!(matches!(err, WebhookUrlError::UnsupportedScheme { .. }));
    }

    #[test]
    fn rejects_unparseable_strings() {
        let err = WebhookUrl::new("").unwrap_err();
        assert!(matches!(err, WebhookUrlError::Parse { .. }));
        let err = WebhookUrl::new("not a url").unwrap_err();
        assert!(matches!(err, WebhookUrlError::Parse { .. }));
        let err = WebhookUrl::new("http://").unwrap_err();
        assert!(matches!(err, WebhookUrlError::Parse { .. }));
    }

    #[test]
    fn display_roundtrips_url() {
        let url = WebhookUrl::new("https://example.com/hook").unwrap();
        assert_eq!(url.to_string(), "https://example.com/hook");
    }

    #[test]
    fn error_display_covers_every_variant() {
        let parse_err = WebhookUrl::new("not a url").unwrap_err();
        assert!(parse_err.to_string().contains("not a valid URL"));

        let scheme_err = WebhookUrl::new("ftp://bad").unwrap_err();
        assert!(scheme_err.to_string().contains("ftp://"));
        assert!(scheme_err.to_string().contains("must use http"));
    }

    #[test]
    fn as_str_returns_inner_value() {
        let url = WebhookUrl::new("https://example.com/hook").unwrap();
        assert_eq!(url.as_str(), "https://example.com/hook");
    }
}
