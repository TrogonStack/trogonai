use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use trogon_cron_jobs_proto::v1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageHeadersError {
    InvalidName { name: String },
    InvalidValue { name: String },
}

impl std::fmt::Display for MessageHeadersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName { name } => write!(f, "header name '{name}' is invalid"),
            Self::InvalidValue { name } => write!(f, "header '{name}' contains an invalid value"),
        }
    }
}

impl std::error::Error for MessageHeadersError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageHeaders(Vec<(String, String)>);

impl MessageHeaders {
    pub fn new<I, N, V>(input: I) -> Result<Self, MessageHeadersError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        let mut headers: Vec<(String, String)> = Vec::new();
        for (name, value) in input {
            let name: String = name.into();
            let value: String = value.into();
            if name.trim().is_empty()
                || name.contains(':')
                || name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
            {
                return Err(MessageHeadersError::InvalidName { name });
            }
            if value.chars().any(|ch| ch == '\r' || ch == '\n' || ch == '\0') {
                return Err(MessageHeadersError::InvalidValue { name });
            }
            headers.push((name, value));
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
        let headers = Vec::<(String, String)>::deserialize(deserializer)?;
        Self::new(headers).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MessageEnvelope {
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "MessageHeaders::is_empty")]
    pub headers: MessageHeaders,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MessageContent(String);

impl MessageContent {
    pub fn new(content: impl Into<String>) -> Self {
        Self(content.into())
    }

    pub fn from_static(content: &'static str) -> Self {
        Self(content.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for MessageContent {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for MessageContent {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<[u8]> for MessageContent {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl From<&MessageEnvelope> for v1::JobMessage {
    fn from(value: &MessageEnvelope) -> Self {
        let mut message = v1::JobMessage::new();
        message.set_content(value.content.as_str());
        for (name, val) in value.headers.as_slice() {
            let mut header = v1::Header::new();
            header.set_name(name.as_str());
            header.set_value(val.as_str());
            message.headers_mut().push(header);
        }
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_preserve_ordered_pairs() {
        let headers = MessageHeaders::new([("x-kind", "heartbeat"), ("x-kind", "retry"), ("x-owner", "ops")]).unwrap();

        assert_eq!(
            headers.as_slice(),
            &[
                ("x-kind".to_string(), "heartbeat".to_string()),
                ("x-kind".to_string(), "retry".to_string()),
                ("x-owner".to_string(), "ops".to_string()),
            ]
        );
    }

    #[test]
    fn invalid_header_name_is_rejected() {
        let error = MessageHeaders::new([("bad name", "value")]).unwrap_err();
        assert!(error.to_string().contains("invalid"));
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
                prop_assert_eq!(slice[0].0.as_str(), name.as_str());
                prop_assert_eq!(slice[0].1.as_str(), value.as_str());
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
