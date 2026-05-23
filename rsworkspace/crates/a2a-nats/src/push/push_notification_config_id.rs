use std::fmt;

/// Resource id for a [`a2a_types::TaskPushNotificationConfig`] (the `id` field).
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PushNotificationConfigId(String);

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PushNotificationConfigIdError {
    Empty,
}

impl fmt::Display for PushNotificationConfigIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "push notification config id cannot be empty"),
        }
    }
}

impl std::error::Error for PushNotificationConfigIdError {}

impl PushNotificationConfigId {
    pub fn new(raw: impl Into<String>) -> Result<Self, PushNotificationConfigIdError> {
        let s = raw.into();
        let t = s.trim();
        if t.is_empty() {
            return Err(PushNotificationConfigIdError::Empty);
        }
        Ok(Self(t.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PushNotificationConfigId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for PushNotificationConfigId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PushNotificationConfigId").field(&self.0).finish()
    }
}
