//! Clock trait abstraction for mocking time in tests.
//!
//! - `SystemClock`: delegates to real `tokio::time`
//! - `MockClock`: returns a controllable instant, `sleep()` is a no-op

use std::sync::{Arc, Mutex};
use tokio::time::{Duration, Instant};

/// Abstraction over the system clock.
/// Implement this trait to control time in tests.
#[allow(async_fn_in_trait)]
pub trait Clock: Send + Sync + 'static {
    /// Return the current instant.
    fn now(&self) -> Instant;

    /// Sleep for the given duration (no-op in mock implementations).
    async fn sleep(&self, duration: Duration);
}

/// Live implementation: delegates to real tokio time.
#[derive(Clone, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Mock clock for unit tests.
/// - `now()` returns a fixed instant that advances only when you call `advance()`
/// - `sleep()` is a no-op (returns immediately without real delay)
#[derive(Clone)]
pub struct MockClock {
    inner: Arc<Mutex<MockClockInner>>,
}

struct MockClockInner {
    current: Instant,
}

impl MockClock {
    /// Create a new mock clock fixed at `Instant::now()` at construction time.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockClockInner {
                current: Instant::now(),
            })),
        }
    }

    /// Advance the mock clock by `duration`.
    /// Subsequent `now()` calls will reflect the new time.
    pub fn advance(&self, duration: Duration) {
        self.inner.lock().unwrap().current += duration;
    }

    /// Return the current mocked instant.
    pub fn current(&self) -> Instant {
        self.inner.lock().unwrap().current
    }
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MockClock {
    fn now(&self) -> Instant {
        self.inner.lock().unwrap().current
    }

    async fn sleep(&self, _duration: Duration) {
        // No-op: tests don't want real sleeps
    }
}
