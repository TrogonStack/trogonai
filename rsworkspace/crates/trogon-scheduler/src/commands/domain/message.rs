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
mod tests {
    use super::*;

    #[test]
    fn headers_preserve_ordered_pairs() {
        let headers = MessageHeaders::new([("x-kind", "heartbeat"), ("x-kind", "retry"), ("x-owner", "ops")]).unwrap();

        assert_eq!(
            headers
                .as_slice()
                .iter()
                .map(|header| (header.name().as_str(), header.value().as_str()))
                .collect::<Vec<_>>(),
            vec![("x-kind", "heartbeat"), ("x-kind", "retry"), ("x-owner", "ops")]
        );
    }

    #[test]
    fn invalid_header_name_is_rejected() {
        let error = MessageHeaders::new([("bad name", "value")]).unwrap_err();
        assert!(error.to_string().contains("invalid"));
    }

    #[test]
    fn message_conversion_uses_content_type_from_message_content() {
        let message = v1::Message::from(&MessageEnvelope {
            content: MessageContent::with_content_type("plain text", "text/plain"),
            headers: MessageHeaders::default(),
        });

        assert_eq!(message.content.as_option().unwrap().content_type, "text/plain");
    }

    #[test]
    fn header_value_objects_cover_conversions_and_accessors() {
        let name = HeaderName::try_from("x-kind".to_string()).unwrap();
        let value = HeaderValue::try_from("heartbeat".to_string()).unwrap();
        let header = MessageHeader {
            name: name.clone(),
            value: value.clone(),
        };

        assert_eq!(name.as_str(), "x-kind");
        assert_eq!(name.into_string(), "x-kind");
        assert_eq!(value.as_str(), "heartbeat");
        assert_eq!(value.into_string(), "heartbeat");
        assert_eq!(
            header.into_pair(),
            (
                HeaderName::new("x-kind").unwrap(),
                HeaderValue::new("heartbeat").unwrap()
            )
        );
        assert!(HeaderName::try_from("bad name").is_err());
        assert!(HeaderValue::try_from("bad\nvalue").is_err());
    }

    #[test]
    fn message_headers_cover_collection_helpers_and_errors() {
        let headers = MessageHeaders::try_from(vec![("x-kind".to_string(), "heartbeat".to_string())]).unwrap();
        let header_vec = headers.clone().into_vec();

        assert!(!headers.is_empty());
        assert_eq!(header_vec.len(), 1);
        assert_eq!(
            MessageHeadersError::InvalidName {
                name: "bad name".to_string()
            }
            .to_string(),
            "header name 'bad name' is invalid"
        );
        assert_eq!(
            MessageHeaders::new([("x-kind", "bad\rvalue")]).unwrap_err().to_string(),
            "header 'x-kind' contains an invalid value"
        );
    }

    #[test]
    fn message_content_covers_constructors_and_byte_access() {
        let octets = MessageContent::from_static("raw");
        let plain = MessageContent::with_content_type("plain", "text/plain");
        let json = MessageContent::json("{}");
        let from_string = MessageContent::from("owned".to_string());
        let from_str = MessageContent::from("borrowed");

        assert_eq!(octets.content_type().as_str(), "application/octet-stream");
        assert_eq!(octets.as_ref(), b"raw");
        assert_eq!(plain.content_type().as_str(), "text/plain");
        assert_eq!(plain.as_str(), "plain");
        assert_eq!(json.content_type().as_str(), "application/json");
        assert_eq!(from_string.into_string(), "owned");
        assert_eq!(from_str.as_slice(), b"borrowed");
    }

    #[test]
    fn message_conversion_includes_content_and_headers() {
        let message = v1::Message::from(&MessageEnvelope {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: MessageHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        });

        let content = message.content.as_option().unwrap();
        assert_eq!(content.content_type, "application/json");
        assert_eq!(content.data, br#"{"ok":true}"#);
        assert_eq!(message.headers.len(), 1);
        assert_eq!(message.headers[0].name, "x-kind");
        assert_eq!(message.headers[0].value, "heartbeat");
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn message_headers_accept_valid_name_value_pairs(
                name in "x-[a-z]{1,16}",
                value in "[ -~]{0,32}",
            ) {
                let headers = MessageHeaders::new([(name.as_str(), value.as_str())]).unwrap();
                let slice = headers.as_slice();
                prop_assert_eq!(slice.len(), 1);
                prop_assert_eq!(slice[0].name().as_str(), name.as_str());
                prop_assert_eq!(slice[0].value().as_str(), value.as_str());
            }

            #[test]
            fn message_headers_reject_name_containing_colon(
                prefix in "[a-z]{1,8}",
                suffix in "[a-z]{0,8}",
                value in "[ -~]{0,16}",
            ) {
                let name = format!("{prefix}:{suffix}");
                prop_assert!(MessageHeaders::new([(name, value)]).is_err());
            }

            #[test]
            fn message_headers_reject_name_containing_whitespace_or_control(
                prefix in "[a-z]{1,8}",
                bad in prop_oneof![Just(' '), Just('\t'), Just('\n'), Just('\r'), Just('\0')],
                suffix in "[a-z]{0,8}",
                value in "[ -~]{0,16}",
            ) {
                let name = format!("{prefix}{bad}{suffix}");
                prop_assert!(MessageHeaders::new([(name, value)]).is_err());
            }

            #[test]
            fn message_headers_reject_value_containing_crlf_or_null(
                name in "x-[a-z]{1,8}",
                prefix in "[ -~]{0,8}",
                bad in prop_oneof![Just('\r'), Just('\n'), Just('\0')],
                suffix in "[ -~]{0,8}",
            ) {
                let value = format!("{prefix}{bad}{suffix}");
                prop_assert!(MessageHeaders::new([(name, value)]).is_err());
            }
        }
    }
}
