use async_nats::header::IntoHeaderName;
use async_nats::{HeaderMap, HeaderValue};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderText(HeaderValue);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("must not contain carriage return or line feed")]
pub struct InvalidHeaderText;

impl HeaderText {
    pub fn new(value: &str) -> Result<Self, InvalidHeaderText> {
        value.parse::<HeaderValue>().map(Self).map_err(|_| InvalidHeaderText)
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

    pub fn into_inner(self) -> HeaderMap {
        self.0
    }
}

#[cfg(test)]
mod tests;
