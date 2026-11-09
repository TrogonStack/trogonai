#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MessageId(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid message identifier")]
pub struct MessageIdError;

impl MessageId {
    pub fn new(value: impl Into<String>) -> Result<Self, MessageIdError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(MessageIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
