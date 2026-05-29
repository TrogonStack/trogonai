use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use super::errors::ContextThrottleError;
use super::key::ContextThrottleKey;
use super::state::{AcquireResult, Clock, SystemClock, TokenBucket};

/// Result of a context throttle acquire attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextThrottleOutcome {
    Allowed,
    Throttled { retry_after_ms: u64 },
}

/// In-process token-bucket store keyed by `(tenant_id, agent_id, purpose)`.
pub struct ContextThrottle<C: Clock = SystemClock> {
    config: super::ContextThrottleConfig,
    clock: C,
    buckets: Mutex<HashMap<ContextThrottleKey, TokenBucket>>,
}

impl ContextThrottle<SystemClock> {
    #[must_use]
    pub fn new(config: super::ContextThrottleConfig) -> Self {
        Self::with_clock(config, SystemClock)
    }
}

impl<C: Clock> ContextThrottle<C> {
    #[must_use]
    pub fn with_clock(config: super::ContextThrottleConfig, clock: C) -> Self {
        Self {
            config,
            clock,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn acquire(
        &self,
        key: &ContextThrottleKey,
        cost: u32,
    ) -> Result<ContextThrottleOutcome, ContextThrottleError> {
        let budget = self.config.budget_for(key);
        let now = self.clock.now();
        let mut map = self.buckets.lock().expect("context throttle map lock");
        let bucket = map
            .entry(key.clone())
            .or_insert_with(|| TokenBucket::new(budget, now));
        if bucket.budget != budget {
            *bucket = TokenBucket::new(budget, now);
        }
        match bucket.try_acquire(now, cost)? {
            AcquireResult::Allowed => Ok(ContextThrottleOutcome::Allowed),
            AcquireResult::Throttled { retry_after_ms } => {
                Ok(ContextThrottleOutcome::Throttled { retry_after_ms })
            }
        }
    }

    /// Drops buckets that have not been touched within `idle_for`. Not invoked from `acquire`.
    pub fn evict_idle(&self, idle_for: Duration) -> usize {
        let now = self.clock.now();
        let mut map = self.buckets.lock().expect("context throttle map lock");
        let before = map.len();
        map.retain(|_, bucket| now.duration_since(bucket.last_touched()) < idle_for);
        before - map.len()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::context_throttle::budget::ContextBudget;
    use crate::context_throttle::state::TestClock;

    fn key(label: &str) -> ContextThrottleKey {
        ContextThrottleKey::new("acme", "agent/oncall", label).expect("key")
    }

    #[test]
    fn first_acquire_allowed_and_consumes_token() {
        let clock = TestClock::new(Instant::now());
        let throttle = ContextThrottle::with_clock(
            super::super::ContextThrottleConfig::default(),
            clock,
        );
        let k = key("deploy");
        assert_eq!(
            throttle.acquire(&k, 1).expect("acquire"),
            ContextThrottleOutcome::Allowed
        );
        for _ in 0..29 {
            assert_eq!(
                throttle.acquire(&k, 1).expect("acquire"),
                ContextThrottleOutcome::Allowed
            );
        }
        assert!(matches!(
            throttle.acquire(&k, 1).expect("acquire"),
            ContextThrottleOutcome::Throttled { .. }
        ));
    }

    #[test]
    fn burst_without_time_yields_throttled_with_retry_hint() {
        let clock = TestClock::new(Instant::now());
        let config = super::super::ContextThrottleConfig {
            default_budget: ContextBudget {
                tokens_per_minute: 60,
                burst: 2,
            },
            overrides: HashMap::new(),
        };
        let throttle = ContextThrottle::with_clock(config, clock);
        let k = key("burst");
        assert_eq!(
            throttle.acquire(&k, 1).expect("first"),
            ContextThrottleOutcome::Allowed
        );
        assert_eq!(
            throttle.acquire(&k, 1).expect("second"),
            ContextThrottleOutcome::Allowed
        );
        let outcome = throttle.acquire(&k, 1).expect("third");
        match outcome {
            ContextThrottleOutcome::Throttled { retry_after_ms } => {
                assert!(retry_after_ms >= 1);
            }
            ContextThrottleOutcome::Allowed => panic!("expected throttle after burst"),
        }
    }

    #[test]
    fn advance_clock_refills_proportionally() {
        let clock = TestClock::new(Instant::now());
        let config = super::super::ContextThrottleConfig {
            default_budget: ContextBudget {
                tokens_per_minute: 60,
                burst: 2,
            },
            overrides: HashMap::new(),
        };
        let throttle = ContextThrottle::with_clock(config, clock.clone());
        let k = key("refill");
        let _ = throttle.acquire(&k, 2);
        assert!(matches!(
            throttle.acquire(&k, 1).expect("blocked"),
            ContextThrottleOutcome::Throttled { .. }
        ));
        clock.advance(Duration::from_secs(1));
        assert_eq!(
            throttle.acquire(&k, 1).expect("refilled"),
            ContextThrottleOutcome::Allowed
        );
    }

    #[test]
    fn per_tenant_agent_override_beats_default() {
        let clock = TestClock::new(Instant::now());
        let mut overrides = HashMap::new();
        overrides.insert(
            ("acme".into(), "agent/oncall".into()),
            ContextBudget {
                tokens_per_minute: 60,
                burst: 5,
            },
        );
        let config = super::super::ContextThrottleConfig {
            default_budget: ContextBudget {
                tokens_per_minute: 60,
                burst: 2,
            },
            overrides,
        };
        let throttle = ContextThrottle::with_clock(config, clock);
        let k = key("override");
        for _ in 0..5 {
            assert_eq!(
                throttle.acquire(&k, 1).expect("acquire"),
                ContextThrottleOutcome::Allowed
            );
        }
        assert!(matches!(
            throttle.acquire(&k, 1).expect("sixth"),
            ContextThrottleOutcome::Throttled { .. }
        ));
    }

    #[test]
    fn evict_idle_removes_stale_bucket_keeps_active() {
        let clock = TestClock::new(Instant::now());
        let throttle = ContextThrottle::with_clock(
            super::super::ContextThrottleConfig::default(),
            clock.clone(),
        );
        let stale = key("stale");
        let active = key("active");
        let _ = throttle.acquire(&stale, 1);
        clock.advance(Duration::from_secs(120));
        let _ = throttle.acquire(&active, 1);
        let evicted = throttle.evict_idle(Duration::from_secs(60));
        assert_eq!(evicted, 1);
        assert_eq!(
            throttle.acquire(&active, 1).expect("active still present"),
            ContextThrottleOutcome::Allowed
        );
    }

    #[tokio::test]
    async fn distinct_keys_do_not_share_budgets() {
        let clock = TestClock::new(Instant::now());
        let throttle = std::sync::Arc::new(ContextThrottle::with_clock(
            super::super::ContextThrottleConfig {
                default_budget: ContextBudget {
                    tokens_per_minute: 60,
                    burst: 3,
                },
                overrides: HashMap::new(),
            },
            clock,
        ));
        let key_a = key("a");
        let key_b = key("b");
        let mut handles = Vec::new();
        for i in 0..10 {
            let t = std::sync::Arc::clone(&throttle);
            let k = if i % 2 == 0 { key_a.clone() } else { key_b.clone() };
            handles.push(tokio::spawn(async move {
                t.acquire(&k, 1).expect("acquire")
            }));
        }
        let mut allowed_a = 0;
        let mut allowed_b = 0;
        for (i, handle) in handles.into_iter().enumerate() {
            let outcome = handle.await.expect("join");
            if outcome == ContextThrottleOutcome::Allowed {
                if i % 2 == 0 {
                    allowed_a += 1;
                } else {
                    allowed_b += 1;
                }
            }
        }
        assert_eq!(allowed_a, 3);
        assert_eq!(allowed_b, 3);
    }
}
