//! Test-substitutable wall clock for token freshness checks.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Pluggable time source. Tests use a fixed `MockTimeSource`.
pub trait TimeSource: Send + Sync {
    /// Unix seconds.
    fn now(&self) -> i64;
}

/// Real wall clock.
#[derive(Clone, Default)]
pub struct SystemTimeSource;

impl TimeSource for SystemTimeSource {
    fn now(&self) -> i64 {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        i64::try_from(secs).unwrap_or(i64::MAX)
    }
}

impl<T: TimeSource + ?Sized> TimeSource for Arc<T> {
    fn now(&self) -> i64 {
        (**self).now()
    }
}
