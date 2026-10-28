use async_nats::header::IntoHeaderName;
use async_nats::{HeaderMap, HeaderValue};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderText(HeaderValue);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("must not contain carriage return or line feed")]
pub struct InvalidHeaderTextError;

impl HeaderText {
    pub fn new(value: &str) -> Result<Self, InvalidHeaderTextError> {
        value
            .parse::<HeaderValue>()
            .map(Self)
            .map_err(|_| InvalidHeaderTextError)
    }

    pub fn integer(value: i64) -> Self {
        Self(HeaderValue::from(value.to_string()))
    }

    pub fn unknown() -> Self {
        Self(HeaderValue::from("unknown"))
    }

    pub fn or_unknown(value: &str) -> Self {
        Self::new(value).unwrap_or_else(|_| Self::unknown())
    }

    pub fn digest(prefix: &str, digest: &[u8]) -> Self {
        Self(HeaderValue::from(format!("{prefix}{}", hex::encode(digest))))
    }
}

#[derive(Debug, Default)]
pub struct DeliveryHeaders(HeaderMap);

impl DeliveryHeaders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: impl IntoHeaderName, value: &HeaderText) -> Self {
        self.0.insert(name, value.0.clone());
        self
    }

    pub fn with_static(mut self, name: impl IntoHeaderName, value: &'static str) -> Self {
        self.0.insert(name, value);
        self
    }

    pub fn with_optional(self, name: impl IntoHeaderName, value: Option<&str>) -> Result<Self, InvalidHeaderTextError> {
        match value {
            Some(value) => Ok(self.with(name, &HeaderText::new(value)?)),
            None => Ok(self),
        }
    }

    pub fn with_optional_text(self, name: impl IntoHeaderName, value: Option<&HeaderText>) -> Self {
        match value {
            Some(value) => self.with(name, value),
            None => self,
        }
    }

    pub fn into_inner(self) -> HeaderMap {
        self.0
    }
}

#[cfg(test)]
mod tests;
