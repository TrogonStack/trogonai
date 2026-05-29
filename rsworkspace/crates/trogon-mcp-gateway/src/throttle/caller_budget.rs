use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CallerKey {
    pub tenant: String,
    pub sub: String,
}

#[derive(Debug)]
struct SlidingWindowMs {
    window: Duration,
    limit: u32,
    events: VecDeque<Instant>,
}

impl SlidingWindowMs {
    fn new(window: Duration, limit: u32) -> Self {
        Self {
            window,
            limit,
            events: VecDeque::new(),
        }
    }

    fn purge_before(&mut self, now: Instant) {
        while self
            .events
            .front()
            .is_some_and(|ts| now.duration_since(*ts) >= self.window)
        {
            self.events.pop_front();
        }
    }

    /// Returns `Ok(())` when admitted; `Err(retry_after_ms)` when budget exceeded.
    fn try_consume(&mut self, now: Instant) -> Result<(), u64> {
        self.purge_before(now);
        if self.events.len() >= self.limit as usize {
            let oldest = self.events.front().copied().unwrap_or(now);
            let elapsed_ms = now.duration_since(oldest).as_millis() as u64;
            let window_ms = self.window.as_millis().max(1) as u64;
            let retry_after_ms = window_ms.saturating_sub(elapsed_ms).max(1);
            return Err(retry_after_ms);
        }
        self.events.push_back(now);
        Ok(())
    }
}

#[derive(Debug)]
pub struct CallerBudgetRegistry {
    window: Duration,
    limit: u32,
    buckets: Mutex<HashMap<CallerKey, SlidingWindowMs>>,
}

impl CallerBudgetRegistry {
    pub fn new(window: Duration, limit: u32) -> Self {
        Self {
            window,
            limit,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn try_consume(&self, key: &CallerKey) -> Result<(), u64> {
        let now = Instant::now();
        let mut map = self.buckets.lock().expect("caller budget map lock");
        let bucket = map
            .entry(key.clone())
            .or_insert_with(|| SlidingWindowMs::new(self.window, self.limit));
        bucket.try_consume(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice() -> CallerKey {
        CallerKey {
            tenant: "acme".into(),
            sub: "user:alice".into(),
        }
    }

    fn bob() -> CallerKey {
        CallerKey {
            tenant: "acme".into(),
            sub: "user:bob".into(),
        }
    }

    #[test]
    fn fourth_request_denied_within_window() {
        let registry = CallerBudgetRegistry::new(Duration::from_secs(1), 3);
        let key = alice();
        assert!(registry.try_consume(&key).is_ok());
        assert!(registry.try_consume(&key).is_ok());
        assert!(registry.try_consume(&key).is_ok());
        let err = registry.try_consume(&key).unwrap_err();
        assert!(err > 0);
    }

    #[test]
    fn distinct_subs_do_not_share_budget() {
        let registry = CallerBudgetRegistry::new(Duration::from_secs(1), 3);
        for _ in 0..3 {
            assert!(registry.try_consume(&alice()).is_ok());
        }
        assert!(registry.try_consume(&alice()).is_err());
        assert!(registry.try_consume(&bob()).is_ok());
    }

    #[test]
    fn budget_refills_after_window() {
        let registry = CallerBudgetRegistry::new(Duration::from_millis(50), 1);
        let key = alice();
        assert!(registry.try_consume(&key).is_ok());
        assert!(registry.try_consume(&key).is_err());
        std::thread::sleep(Duration::from_millis(60));
        assert!(registry.try_consume(&key).is_ok());
    }

    #[test]
    fn retry_after_ms_reflects_window_remainder() {
        let registry = CallerBudgetRegistry::new(Duration::from_millis(100), 1);
        let key = alice();
        assert!(registry.try_consume(&key).is_ok());
        std::thread::sleep(Duration::from_millis(30));
        let retry = registry.try_consume(&key).unwrap_err();
        assert!(retry > 0 && retry <= 100);
    }
}
