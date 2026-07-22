use std::fmt;

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookUrl(Url);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebhookUrlError {
    /// Couldn't parse the input as a URL at all.
    #[error("webhook URL is not a valid URL ({reason}): {raw}")]
    Parse { raw: String, reason: String },
    /// Parsed cleanly but the scheme isn't `http`/`https`.
    #[error("webhook URL must use http:// or https://, got {scheme}://: {raw}")]
    UnsupportedScheme { raw: String, scheme: String },
}

impl WebhookUrl {
    pub fn new(raw: impl Into<String>) -> Result<Self, WebhookUrlError> {
        let raw = raw.into();
        let parsed = Url::parse(&raw).map_err(|e| WebhookUrlError::Parse {
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

#[cfg(test)]
mod tests;
