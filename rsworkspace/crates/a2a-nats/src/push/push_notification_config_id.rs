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
mod tests {
    use super::*;

    #[test]
    fn new_trims_and_accepts_non_empty() {
        let id = PushNotificationConfigId::new("  cfg-1 ").unwrap();
        assert_eq!(id.as_str(), "cfg-1");
        assert_eq!(id.to_string(), "cfg-1");
    }

    #[test]
    fn new_rejects_empty_and_blank() {
        assert_eq!(
            PushNotificationConfigId::new(""),
            Err(PushNotificationConfigIdError::Empty)
        );
        assert_eq!(
            PushNotificationConfigId::new("   "),
            Err(PushNotificationConfigIdError::Empty)
        );
    }

    #[test]
    fn new_accepts_colon_now_that_idempotency_key_is_length_prefixed() {
        let id = PushNotificationConfigId::new("cfg:1").unwrap();
        assert_eq!(id.as_str(), "cfg:1");
    }

    #[test]
    fn error_display_and_debug_render_distinct_messages() {
        let empty = PushNotificationConfigIdError::Empty;
        assert!(empty.to_string().contains("cannot be empty"));
        assert!(format!("{empty:?}").contains("Empty"));
    }

    #[test]
    fn id_debug_reveals_inner_value() {
        let id = PushNotificationConfigId::new("cfg-2").unwrap();
        assert!(format!("{id:?}").contains("cfg-2"));
    }
}
