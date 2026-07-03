use buffa::MessageField;
use trogonai_proto::content::v1alpha1 as content_v1alpha1;
use trogonai_proto::scheduler::schedules::v1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageHeadersError {
    #[error("header name '{name}' is invalid")]
    InvalidName { name: String },
    #[error("header '{name}' contains an invalid value")]
    InvalidValue { name: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageHeaders(Vec<MessageHeader>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHeader {
    name: HeaderName,
    value: HeaderValue,
}

impl MessageHeader {
    pub fn name(&self) -> &HeaderName {
        &self.name
    }

    pub fn value(&self) -> &HeaderValue {
        &self.value
    }

    pub fn into_pair(self) -> (HeaderName, HeaderValue) {
        (self.name, self.value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderNameError {
    name: String,
}

impl HeaderName {
    pub fn new(name: impl Into<String>) -> Result<Self, HeaderNameError> {
        let name = name.into();
        if name.trim().is_empty() || name.contains(':') || name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(HeaderNameError { name });
        }

        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for HeaderName {
    type Error = HeaderNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for HeaderName {
    type Error = HeaderNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderValue(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderValueError;

impl HeaderValue {
    pub fn new(value: impl Into<String>) -> Result<Self, HeaderValueError> {
        let value = value.into();
        if value.chars().any(|ch| ch == '\r' || ch == '\n' || ch == '\0') {
            return Err(HeaderValueError);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for HeaderValue {
    type Error = HeaderValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for HeaderValue {
    type Error = HeaderValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl MessageHeaders {
    pub fn new<I, N, V>(input: I) -> Result<Self, MessageHeadersError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        let mut headers: Vec<MessageHeader> = Vec::new();
        for (name, value) in input {
            let name: String = name.into();
            let value: String = value.into();
            let name =
                HeaderName::new(name).map_err(|source| MessageHeadersError::InvalidName { name: source.name })?;
            let value = HeaderValue::new(value).map_err(|_| MessageHeadersError::InvalidValue {
                name: name.as_str().to_string(),
            })?;
            headers.push(MessageHeader { name, value });
        }

        Ok(Self(headers))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[MessageHeader] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<MessageHeader> {
        self.0
    }
}

impl TryFrom<Vec<(String, String)>> for MessageHeaders {
    type Error = MessageHeadersError;

    fn try_from(value: Vec<(String, String)>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageEnvelope {
    pub content: MessageContent,
    pub headers: MessageHeaders,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageContent {
    content_type: MessageContentType,
    data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageContentType(String);

impl Default for MessageContentType {
    fn default() -> Self {
        Self("application/octet-stream".to_string())
    }
}

impl MessageContentType {
    pub fn new(content_type: impl Into<String>) -> Self {
        Self(content_type.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl MessageContent {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content_type: MessageContentType::default(),
            data: content.into(),
        }
    }

    pub fn with_content_type(content: impl Into<String>, content_type: impl Into<String>) -> Self {
        Self {
            content_type: MessageContentType::new(content_type),
            data: content.into(),
        }
    }

    pub fn json(content: impl Into<String>) -> Self {
        Self::with_content_type(content, "application/json")
    }

    pub fn from_static(content: &'static str) -> Self {
        Self::new(content)
    }

    pub fn content_type(&self) -> &MessageContentType {
        &self.content_type
    }

    pub fn as_str(&self) -> &str {
        &self.data
    }

    pub fn as_slice(&self) -> &[u8] {
        self.data.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.data
    }
}

impl From<String> for MessageContent {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for MessageContent {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl AsRef<[u8]> for MessageContent {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl From<&MessageEnvelope> for v1::Message {
    fn from(value: &MessageEnvelope) -> Self {
        v1::Message {
            content: MessageField::some(content_v1alpha1::Content {
                content_type: value.content.content_type().as_str().to_string(),
                data: value.content.as_slice().to_vec(),
            }),
            headers: value
                .headers
                .as_slice()
                .iter()
                .map(|header| v1::Header {
                    name: header.name().as_str().to_string(),
                    value: header.value().as_str().to_string(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests;
