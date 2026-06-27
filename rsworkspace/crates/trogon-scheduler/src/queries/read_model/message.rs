use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageHeadersError {
    #[error("header name '{name}' is invalid")]
    InvalidName { name: String },
    #[error("header '{name}' contains an invalid value")]
    InvalidValue { name: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageHeaders(Vec<(String, String)>);

impl MessageHeaders {
    pub fn new<I, N, V>(headers: I) -> Result<Self, MessageHeadersError>
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
                return Err(MessageHeadersError::InvalidName { name: name.clone() });
            }
            if value.chars().any(|ch| ch == '\r' || ch == '\n' || ch == '\0') {
                return Err(MessageHeadersError::InvalidValue { name: name.clone() });
            }
        }

        Ok(Self(headers))
    }

    pub fn from_pairs<I, N, V>(headers: I) -> Self
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        Self(
            headers
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        )
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
    type Error = MessageHeadersError;

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
        // Read back exactly what the projection wrote. The projection stores
        // headers verbatim (`from_pairs`) since the command layer already
        // validated them, so deserialization must not re-validate — otherwise a
        // header a stricter validator would reject becomes a permanently
        // unreadable KV entry.
        let headers = Vec::<(String, String)>::deserialize(deserializer)?;
        Ok(Self::from_pairs(headers))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MessageEnvelope {
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "MessageHeaders::is_empty")]
    pub headers: MessageHeaders,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MessageContent {
    /// The payload's media type (e.g. `application/json`), preserved from the
    /// event so the read model reflects exactly what will be delivered.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_type: String,
    /// The payload. The scheduler treats message content as UTF-8 text (the
    /// executor rejects non-UTF-8), so the read model stores it as a string.
    pub body: String,
}

impl MessageContent {
    pub fn new(content_type: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            content_type: content_type.into(),
            body: body.into(),
        }
    }

    pub fn from_static(content_type: &'static str, body: &'static str) -> Self {
        Self::new(content_type, body)
    }

    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    pub fn as_str(&self) -> &str {
        &self.body
    }

    pub fn as_slice(&self) -> &[u8] {
        self.body.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.body
    }
}

impl AsRef<[u8]> for MessageContent {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(test)]
mod tests;
