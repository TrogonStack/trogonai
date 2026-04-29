use std::time::Instant;

/// In-process cache entry for a single vault key.
///
/// During key rotation, `previous` holds the old value until the grace period
/// expires. `resolve_with_previous()` can use it as a fallback if the upstream
/// API rejects the current key with 401.
pub struct RotationSlot {
    pub current: String,
    /// Old value and the instant at which it expires (i.e. `Instant::now() + grace_period`).
    pub previous: Option<(String, Instant)>,
}

impl RotationSlot {
    pub fn new(current: String) -> Self {
        Self { current, previous: None }
    }

    /// Return the previous value if it has not yet expired.
    pub fn valid_previous(&self) -> Option<&str> {
        self.previous
            .as_ref()
            .filter(|(_, expires)| Instant::now() < *expires)
            .map(|(v, _)| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn new_slot_has_no_previous() {
        let slot = RotationSlot::new("key-v1".to_string());
        assert_eq!(slot.current, "key-v1");
        assert!(slot.valid_previous().is_none());
    }

    #[test]
    fn valid_previous_returns_value_within_grace() {
        let mut slot = RotationSlot::new("key-v2".to_string());
        slot.previous = Some(("key-v1".to_string(), Instant::now() + Duration::from_secs(60)));
        assert_eq!(slot.valid_previous(), Some("key-v1"));
    }

    #[test]
    fn valid_previous_returns_none_after_expiry() {
        let mut slot = RotationSlot::new("key-v2".to_string());
        // Expired one second ago.
        slot.previous = Some((
            "key-v1".to_string(),
            Instant::now() - Duration::from_secs(1),
        ));
        assert!(slot.valid_previous().is_none());
    }
}
