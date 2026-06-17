use std::fmt;

/// Resource id for a [`a2a::types::TaskPushNotificationConfig`] (the `id` field).
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PushNotificationConfigId(String);

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PushNotificationConfigIdError {
    Empty,
    /// `:` is reserved as the field separator inside `PushIdempotencyKey`'s
    /// derived form (`{task}:{cfg}:{terminal}`); allowing it inside a config
    /// id would let two distinct (task, cfg) pairs hash to the same key.
    ContainsReservedSeparator,
}

impl fmt::Display for PushNotificationConfigIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "push notification config id cannot be empty"),
            Self::ContainsReservedSeparator => {
                write!(
                    f,
                    "push notification config id must not contain ':' (reserved separator)"
                )
            }
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
        if t.contains(':') {
            return Err(PushNotificationConfigIdError::ContainsReservedSeparator);
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
    fn new_rejects_colon_to_keep_idempotency_key_injective() {
        assert_eq!(
            PushNotificationConfigId::new("cfg:1"),
            Err(PushNotificationConfigIdError::ContainsReservedSeparator)
        );
    }

    #[test]
    fn error_display_and_debug_render_distinct_messages() {
        let empty = PushNotificationConfigIdError::Empty;
        assert!(empty.to_string().contains("cannot be empty"));
        assert!(format!("{empty:?}").contains("Empty"));
        let colon = PushNotificationConfigIdError::ContainsReservedSeparator;
        assert!(colon.to_string().contains("must not contain ':'"));
        assert!(format!("{colon:?}").contains("ContainsReservedSeparator"));
    }

    #[test]
    fn id_debug_reveals_inner_value() {
        let id = PushNotificationConfigId::new("cfg-2").unwrap();
        assert!(format!("{id:?}").contains("cfg-2"));
    }
}
