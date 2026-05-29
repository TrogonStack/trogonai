use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::budget::ContextBudget;
use super::errors::ContextThrottleError;

/// Monotonic time source for lazy token-bucket refill.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Production clock backed by `Instant::now()`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Deterministic clock for unit and integration tests.
#[derive(Clone, Debug)]
pub struct TestClock {
    now: Arc<Mutex<Instant>>,
}

impl TestClock {
    #[must_use]
    pub fn new(start: Instant) -> Self {
        Self {
            now: Arc::new(Mutex::new(start)),
        }
    }

    pub fn advance(&self, delta: Duration) {
        let mut guard = self.now.lock().expect("test clock lock");
        *guard += delta;
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        *self.now.lock().expect("test clock lock")
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    last_touched: Instant,
    pub(crate) budget: ContextBudget,
}

impl TokenBucket {
    pub(crate) fn new(budget: ContextBudget, now: Instant) -> Self {
        Self {
            tokens: budget.capacity(),
            last_refill: now,
            last_touched: now,
            budget,
        }
    }

    pub(crate) fn last_touched(&self) -> Instant {
        self.last_touched
    }

    pub(crate) fn try_acquire(
        &mut self,
        now: Instant,
        cost: u32,
    ) -> Result<AcquireResult, ContextThrottleError> {
        if now < self.last_refill {
            return Err(ContextThrottleError::ClockSkew);
        }
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let capacity = self.budget.capacity();
        let refill = self.budget.refill_per_sec();
        self.tokens = (self.tokens + elapsed * refill).min(capacity);
        self.last_refill = now;
        self.last_touched = now;

        let cost_f = f64::from(cost);
        if self.tokens >= cost_f {
            self.tokens -= cost_f;
            return Ok(AcquireResult::Allowed);
        }

        let deficit = cost_f - self.tokens;
        let retry_after_ms = if refill > 0.0 {
            ((deficit / refill) * 1000.0).ceil() as u64
        } else {
            u64::MAX
        };
        Ok(AcquireResult::Throttled {
            retry_after_ms: retry_after_ms.max(1),
        })
    }
}

pub(crate) enum AcquireResult {
    Allowed,
    Throttled { retry_after_ms: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refill_after_one_second() {
        let start = Instant::now();
        let clock = TestClock::new(start);
        let budget = ContextBudget {
            tokens_per_minute: 60,
            burst: 2,
        };
        let mut bucket = TokenBucket::new(budget, clock.now());
        assert!(matches!(
            bucket.try_acquire(clock.now(), 2).expect("acquire"),
            AcquireResult::Allowed
        ));
        assert!(matches!(
            bucket.try_acquire(clock.now(), 1).expect("acquire"),
            AcquireResult::Throttled { .. }
        ));
        clock.advance(Duration::from_secs(1));
        assert!(matches!(
            bucket.try_acquire(clock.now(), 1).expect("refill"),
            AcquireResult::Allowed
        ));
    }
}
