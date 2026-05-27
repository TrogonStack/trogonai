//! In-memory token-bucket rate limits for STS exchange.
//!
//! v1 limits are **per process instance**; multi-instance deployments apply
//! independent buckets on each replica until a shared limiter ships.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::StsError;

const WINDOW: Duration = Duration::from_secs(10);
const DEFAULT_WKL_LIMIT: u32 = 100;
const DEFAULT_AGENT_LIMIT: u32 = 500;

#[derive(Debug)]
struct Bucket {
    window_start: Instant,
    count: u32,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            count: 0,
        }
    }

    fn admit(&mut self, now: Instant, limit: u32) -> bool {
        if now.duration_since(self.window_start) >= WINDOW {
            *self = Self::new(now);
        }
        if self.count >= limit {
            return false;
        }
        self.count += 1;
        true
    }
}

#[derive(Debug)]
pub struct RateLimiter {
    wkl_limit: u32,
    agent_limit: u32,
    by_wkl: Mutex<HashMap<String, Bucket>>,
    by_agent: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            wkl_limit: DEFAULT_WKL_LIMIT,
            agent_limit: DEFAULT_AGENT_LIMIT,
            by_wkl: Mutex::new(HashMap::new()),
            by_agent: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, wkl: &str, agent_id: Option<&str>) -> Result<(), StsError> {
        let now = Instant::now();
        {
            let mut map = self.by_wkl.lock().expect("wkl bucket lock");
            let bucket = map.entry(wkl.to_string()).or_insert_with(|| Bucket::new(now));
            if !bucket.admit(now, self.wkl_limit) {
                return Err(StsError::RateLimited(format!(
                    "wkl {wkl} exceeded {}/{}s",
                    self.wkl_limit,
                    WINDOW.as_secs()
                )));
            }
        }
        if let Some(agent_id) = agent_id.filter(|s| !s.is_empty()) {
            let mut map = self.by_agent.lock().expect("agent bucket lock");
            let bucket = map.entry(agent_id.to_string()).or_insert_with(|| Bucket::new(now));
            if !bucket.admit(now, self.agent_limit) {
                return Err(StsError::RateLimited(format!(
                    "agent_id {agent_id} exceeded {}/{}s",
                    self.agent_limit,
                    WINDOW.as_secs()
                )));
            }
        }
        Ok(())
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wkl_limit_blocks_after_threshold() {
        let limiter = RateLimiter::new();
        let wkl = "spiffe://acme/sa/test";
        for _ in 0..DEFAULT_WKL_LIMIT {
            limiter.check(wkl, None).expect("under limit");
        }
        assert!(limiter.check(wkl, None).is_err());
    }
}
