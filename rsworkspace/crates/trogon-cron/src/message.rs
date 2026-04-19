use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::error::JobSpecError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageHeaders(Vec<(String, String)>);

impl MessageHeaders {
    pub fn new<I, N, V>(headers: I) -> Result<Self, JobSpecError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        let headers = headers
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect::<Vec<_>>();

        for (name, value) in &headers {
            if name.trim().is_empty()
                || name.contains(':')
                || name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
            {
                return Err(JobSpecError::InvalidHeaderName { name: name.clone() });
            }
            if value
                .chars()
                .any(|ch| ch == '\r' || ch == '\n' || ch == '\0')
            {
                return Err(JobSpecError::InvalidHeaderValue { name: name.clone() });
            }
        }

        Ok(Self(headers))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[(String, String)] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<(String, String)> {
        self.0
    }
}

impl TryFrom<Vec<(String, String)>> for MessageHeaders {
    type Error = JobSpecError;

    fn try_from(value: Vec<(String, String)>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for MessageHeaders {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MessageHeaders {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let headers = Vec::<(String, String)>::deserialize(deserializer)?;
        Self::new(headers).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageContent(Vec<u8>);

impl MessageContent {
    pub fn new(content: impl Into<Vec<u8>>) -> Self {
        Self(content.into())
    }

    pub fn from_static(content: &'static [u8]) -> Self {
        Self(content.to_vec())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for MessageContent {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl From<&[u8]> for MessageContent {
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

impl AsRef<[u8]> for MessageContent {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Serialize for MessageContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map(Self).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_round_trip_as_ordered_pairs() {
        let headers = MessageHeaders::new([
            ("x-kind", "heartbeat"),
            ("x-kind", "retry"),
            ("x-owner", "ops"),
        ])
        .unwrap();

        let json = serde_json::to_string(&headers).unwrap();
        let decoded: MessageHeaders = serde_json::from_str(&json).unwrap();

        assert_eq!(
            json,
            r#"[["x-kind","heartbeat"],["x-kind","retry"],["x-owner","ops"]]"#
        );
        assert_eq!(decoded, headers);
    }

    #[test]
    fn invalid_header_name_is_rejected() {
        let error = MessageHeaders::new([("bad name", "value")]).unwrap_err();
        assert!(error.to_string().contains("invalid"));
    }
}
