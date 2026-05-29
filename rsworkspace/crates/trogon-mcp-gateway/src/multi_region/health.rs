use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use super::region_id::RegionId;
use super::topology::RegionTopology;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionHealthState {
    Healthy,
    Degraded,
    Unreachable,
}

#[derive(Clone, Debug)]
pub struct RegionHealthConfig {
    pub failure_threshold: u32,
    pub unreachable_threshold: u32,
    pub reset_timeout: Duration,
}

impl Default for RegionHealthConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            unreachable_threshold: 5,
            reset_timeout: Duration::from_secs(30),
        }
    }
}

type NowFn = fn() -> Instant;

#[derive(Debug)]
struct RegionHealthRecord {
    state: RegionHealthState,
    consecutive_failures: u32,
    unreachable_since: Option<Instant>,
}

#[derive(Debug)]
pub struct RegionHealth {
    config: RegionHealthConfig,
    now: NowFn,
    records: RwLock<HashMap<RegionId, RegionHealthRecord>>,
}

impl RegionHealth {
    pub fn new(topology: &RegionTopology, config: RegionHealthConfig) -> Self {
        let mut records = HashMap::new();
        for region in topology.region_ids() {
            records.insert(
                region.clone(),
                RegionHealthRecord {
                    state: RegionHealthState::Healthy,
                    consecutive_failures: 0,
                    unreachable_since: None,
                },
            );
        }
        Self {
            config,
            now: Instant::now,
            records: RwLock::new(records),
        }
    }

    pub fn with_clock(topology: &RegionTopology, config: RegionHealthConfig, now: NowFn) -> Self {
        let mut health = Self::new(topology, config);
        health.now = now;
        health
    }

    pub fn is_healthy(&self, region: &RegionId) -> bool {
        self.state(region) == RegionHealthState::Healthy
    }

    pub fn state(&self, region: &RegionId) -> RegionHealthState {
        let now = (self.now)();
        let mut records = self.records.write().expect("health lock");
        let Some(record) = records.get_mut(region) else {
            return RegionHealthState::Unreachable;
        };
        if record.state == RegionHealthState::Unreachable
            && let Some(since) = record.unreachable_since
            && now.duration_since(since) >= self.config.reset_timeout
        {
            record.state = RegionHealthState::Degraded;
            record.unreachable_since = None;
        }
        record.state
    }

    pub fn record_success(&self, region: &RegionId) {
        let mut records = self.records.write().expect("health lock");
        if let Some(record) = records.get_mut(region) {
            record.consecutive_failures = 0;
            record.unreachable_since = None;
            record.state = RegionHealthState::Healthy;
        }
    }

    pub fn record_failure(&self, region: &RegionId) {
        let now = (self.now)();
        let mut records = self.records.write().expect("health lock");
        let Some(record) = records.get_mut(region) else {
            return;
        };
        record.consecutive_failures = record.consecutive_failures.saturating_add(1);
        if record.consecutive_failures >= self.config.unreachable_threshold {
            record.state = RegionHealthState::Unreachable;
            record.unreachable_since = Some(now);
        } else if record.consecutive_failures >= self.config.failure_threshold {
            record.state = RegionHealthState::Degraded;
        }
    }

    pub fn force_unreachable(&self, region: &RegionId) {
        let now = (self.now)();
        let mut records = self.records.write().expect("health lock");
        if let Some(record) = records.get_mut(region) {
            record.state = RegionHealthState::Unreachable;
            record.unreachable_since = Some(now);
            record.consecutive_failures = self.config.unreachable_threshold;
        }
    }

    pub fn force_healthy(&self, region: &RegionId) {
        let mut records = self.records.write().expect("health lock");
        if let Some(record) = records.get_mut(region) {
            record.state = RegionHealthState::Healthy;
            record.consecutive_failures = 0;
            record.unreachable_since = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    use super::*;
    use crate::multi_region::topology::{RegionEndpoint, RegionTopology};

    static CLOCK_MS: AtomicU64 = AtomicU64::new(0);

    fn test_now() -> Instant {
        let ms = CLOCK_MS.load(Ordering::SeqCst);
        Instant::now() + Duration::from_millis(ms)
    }

    fn advance_clock(ms: u64) {
        CLOCK_MS.fetch_add(ms, Ordering::SeqCst);
    }

    fn topo() -> RegionTopology {
        let home = RegionId::new("us-east").unwrap();
        let west = RegionId::new("us-west").unwrap();
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            home.clone(),
            RegionEndpoint {
                nats_url: "nats://east".into(),
                creds_ref: None,
            },
        );
        endpoints.insert(
            west,
            RegionEndpoint {
                nats_url: "nats://west".into(),
                creds_ref: None,
            },
        );
        RegionTopology::new(home, vec![], endpoints).unwrap()
    }

    #[test]
    fn failures_degrade_then_unreachable() {
        CLOCK_MS.store(0, Ordering::SeqCst);
        let home = RegionId::new("us-east").unwrap();
        let health = RegionHealth::with_clock(
            &topo(),
            RegionHealthConfig {
                failure_threshold: 2,
                unreachable_threshold: 4,
                reset_timeout: Duration::from_secs(10),
            },
            test_now,
        );
        health.record_failure(&home);
        assert_eq!(health.state(&home), RegionHealthState::Healthy);
        health.record_failure(&home);
        assert_eq!(health.state(&home), RegionHealthState::Degraded);
        health.record_failure(&home);
        health.record_failure(&home);
        assert_eq!(health.state(&home), RegionHealthState::Unreachable);
    }

    #[test]
    fn reset_timeout_allows_degraded_probe() {
        CLOCK_MS.store(0, Ordering::SeqCst);
        let home = RegionId::new("us-east").unwrap();
        let health = RegionHealth::with_clock(
            &topo(),
            RegionHealthConfig {
                failure_threshold: 1,
                unreachable_threshold: 2,
                reset_timeout: Duration::from_millis(5),
            },
            test_now,
        );
        health.record_failure(&home);
        health.record_failure(&home);
        assert_eq!(health.state(&home), RegionHealthState::Unreachable);
        advance_clock(10);
        assert_eq!(health.state(&home), RegionHealthState::Degraded);
        health.record_success(&home);
        assert!(health.is_healthy(&home));
    }
}
