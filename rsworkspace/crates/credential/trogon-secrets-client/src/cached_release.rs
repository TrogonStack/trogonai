use std::time::SystemTime;

use super::ReleasedValue;

/// One release a client holds, and when it stops serving it.
#[derive(Clone, Debug)]
pub(crate) struct CachedRelease {
    value: ReleasedValue,
    expires_at: SystemTime,
}

impl CachedRelease {
    pub(crate) const fn new(value: ReleasedValue, expires_at: SystemTime) -> Self {
        Self { value, expires_at }
    }

    pub(crate) const fn value(&self) -> &ReleasedValue {
        &self.value
    }

    pub(crate) fn is_live_at(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }
}
