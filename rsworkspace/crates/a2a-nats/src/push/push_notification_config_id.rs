use std::fmt;

/// Resource id for a [`a2a::types::TaskPushNotificationConfig`] (the `id` field).
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PushNotificationConfigId(String);

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum PushNotificationConfigIdError {
    #[error("push notification config id cannot be empty")]
    Empty,
}

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

#[cfg(test)]
mod tests;
