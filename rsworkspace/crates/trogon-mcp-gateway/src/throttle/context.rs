use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ThrottleKey {
    pub tenant: String,
    pub agent_id: String,
    pub purpose: String,
}

#[derive(Clone, Debug)]
pub struct ThrottleConfig {
    pub enabled: bool,
    pub window_secs: u64,
    pub max_requests: u32,
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_secs: 300,
            max_requests: 100,
        }
    }
}

#[derive(Debug)]
struct SlidingWindow {
    window: Duration,
    limit: u32,
    events: VecDeque<Instant>,
}

impl SlidingWindow {
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

    fn check(&mut self, now: Instant) -> Result<(), u64> {
        self.purge_before(now);
        if self.events.len() >= self.limit as usize {
            let oldest = self.events.front().copied().unwrap_or(now);
            let elapsed = now.duration_since(oldest);
            let retry_after = self.window.saturating_sub(elapsed).as_secs().max(1);
            return Err(retry_after);
        }
        self.events.push_back(now);
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct ContextThrottler {
    config: ThrottleConfig,
    windows: Mutex<HashMap<ThrottleKey, SlidingWindow>>,
}

impl ContextThrottler {
    pub fn new(config: ThrottleConfig) -> Self {
        Self {
            config,
            windows: Mutex::new(HashMap::new()),
        }
    }

    pub fn check_and_record(&self, key: &ThrottleKey) -> Option<u64> {
        if !self.config.enabled {
            return None;
        }
        let window = Duration::from_secs(self.config.window_secs);
        let mut map = self.windows.lock().expect("throttle map lock");
        let bucket = map
            .entry(key.clone())
            .or_insert_with(|| SlidingWindow::new(window, self.config.max_requests));
        bucket.check(Instant::now()).err()
    }

    pub fn config(&self) -> &ThrottleConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(label: &str) -> ThrottleKey {
        ThrottleKey {
            tenant: "acme".into(),
            agent_id: "agent/oncall".into(),
            purpose: label.into(),
        }
    }

    #[test]
    fn sliding_window_blocks_after_limit() {
        let mut window = SlidingWindow::new(Duration::from_secs(60), 3);
        let start = Instant::now();
        assert!(window.check(start).is_ok());
        assert!(window.check(start + Duration::from_millis(1)).is_ok());
        assert!(window.check(start + Duration::from_millis(2)).is_ok());
        let blocked = window.check(start + Duration::from_millis(3)).unwrap_err();
        assert!(blocked >= 1);
    }

    #[test]
    fn window_resets_after_elapsed_time() {
        let mut window = SlidingWindow::new(Duration::from_secs(10), 1);
        let start = Instant::now();
        assert!(window.check(start).is_ok());
        assert!(window.check(start + Duration::from_secs(1)).is_err());
        assert!(window.check(start + Duration::from_secs(11)).is_ok());
    }

    #[test]
    fn throttler_returns_retry_after() {
        let throttler = ContextThrottler::new(ThrottleConfig {
            enabled: true,
            window_secs: 60,
            max_requests: 2,
        });
        let throttle_key = key("deploy");
        assert!(throttler.check_and_record(&throttle_key).is_none());
        assert!(throttler.check_and_record(&throttle_key).is_none());
        assert!(throttler.check_and_record(&throttle_key).is_some());
    }

    #[test]
    fn disabled_throttler_never_blocks() {
        let throttler = ContextThrottler::new(ThrottleConfig {
            enabled: false,
            window_secs: 1,
            max_requests: 1,
        });
        let throttle_key = key("noop");
        for _ in 0..5 {
            assert!(throttler.check_and_record(&throttle_key).is_none());
        }
    }
}
