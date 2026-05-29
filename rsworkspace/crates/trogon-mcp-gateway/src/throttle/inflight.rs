use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Instant;

/// Pinned tenant inflight hint per reference-rate-defaults §3.1.
pub const TENANT_INFLIGHT_RETRY_AFTER_MS: u64 = 1000;

#[derive(Debug)]
struct InflightCounter {
    cap: u32,
    current: u32,
    started_at: VecDeque<Instant>,
}

impl InflightCounter {
    fn new(cap: u32) -> Self {
        Self {
            cap,
            current: 0,
            started_at: VecDeque::new(),
        }
    }

    fn try_acquire(&mut self, now: Instant) -> Result<(), u64> {
        if self.current >= self.cap {
            let retry_after_ms = self
                .started_at
                .front()
                .map(|oldest| now.duration_since(*oldest).as_millis().max(1) as u64)
                .unwrap_or(1);
            return Err(retry_after_ms);
        }
        self.current += 1;
        self.started_at.push_back(now);
        Ok(())
    }

    fn release(&mut self) {
        if self.current == 0 {
            return;
        }
        self.current -= 1;
        self.started_at.pop_front();
    }
}

#[derive(Debug, Default)]
pub struct InflightRegistry {
    counters: Mutex<HashMap<String, InflightCounter>>,
}

impl InflightRegistry {
    pub fn try_acquire(&self, key: &str, cap: u32) -> Result<InflightReleaseKey, u64> {
        let now = Instant::now();
        let mut map = self.counters.lock().expect("inflight map lock");
        let counter = map
            .entry(key.to_string())
            .or_insert_with(|| InflightCounter::new(cap));
        counter.cap = cap;
        counter.try_acquire(now)?;
        Ok(InflightReleaseKey(key.to_string()))
    }

    pub fn release(&self, key: &str) {
        let mut map = self.counters.lock().expect("inflight map lock");
        if let Some(counter) = map.get_mut(key) {
            counter.release();
        }
    }
}

#[derive(Debug, Clone)]
pub struct InflightReleaseKey(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn acquire_and_release_frees_slot() {
        let registry = Arc::new(InflightRegistry::default());
        let key = "server-a";
        let k1 = registry.try_acquire(key, 2).expect("first");
        let _k2 = registry.try_acquire(key, 2).expect("second");
        assert!(registry.try_acquire(key, 2).is_err());
        registry.release(&k1.0);
        assert!(registry.try_acquire(key, 2).is_ok());
    }

    #[test]
    fn server_retry_after_ms_from_oldest_inflight() {
        let registry = Arc::new(InflightRegistry::default());
        let _g1 = registry.try_acquire("srv", 1).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let err = registry.try_acquire("srv", 1).unwrap_err();
        assert!(err >= 1);
    }

    #[test]
    fn distinct_keys_isolated() {
        let registry = Arc::new(InflightRegistry::default());
        let _a = registry.try_acquire("a", 1).unwrap();
        assert!(registry.try_acquire("b", 1).is_ok());
    }
}
