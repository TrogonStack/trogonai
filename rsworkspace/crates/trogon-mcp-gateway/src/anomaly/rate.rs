use std::collections::HashMap;

const DEFAULT_WINDOW_MINUTES: i64 = 60;
const DEFAULT_SPIKE_MULTIPLIER: f64 = 3.0;

#[derive(Clone, Debug)]
pub struct RateTracker {
    window_minutes: i64,
    spike_multiplier: f64,
    minute_counts: HashMap<String, HashMap<i64, u32>>,
}

impl Default for RateTracker {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_MINUTES, DEFAULT_SPIKE_MULTIPLIER)
    }
}

impl RateTracker {
    #[must_use]
    pub fn new(window_minutes: i64, spike_multiplier: f64) -> Self {
        Self {
            window_minutes,
            spike_multiplier,
            minute_counts: HashMap::new(),
        }
    }

    pub fn record(&mut self, agent_id: &str, ts_unix_ms: i64) -> u32 {
        let minute = minute_bucket(ts_unix_ms);
        let agent_buckets = self.minute_counts.entry(agent_id.to_string()).or_default();
        let count = agent_buckets.entry(minute).or_insert(0);
        *count += 1;
        *count
    }

    #[must_use]
    pub fn exchange_rate_per_min(&self, agent_id: &str, ts_unix_ms: i64) -> u32 {
        let minute = minute_bucket(ts_unix_ms);
        self.minute_counts
            .get(agent_id)
            .and_then(|buckets| buckets.get(&minute).copied())
            .unwrap_or(0)
    }

    #[must_use]
    pub fn is_spike(&self, agent_id: &str, ts_unix_ms: i64) -> bool {
        let current = self.exchange_rate_per_min(agent_id, ts_unix_ms);
        if current == 0 {
            return false;
        }
        let average = self.moving_average_per_min(agent_id, ts_unix_ms);
        current as f64 > self.spike_multiplier * average
    }

    fn moving_average_per_min(&self, agent_id: &str, ts_unix_ms: i64) -> f64 {
        let current_minute = minute_bucket(ts_unix_ms);
        let Some(buckets) = self.minute_counts.get(agent_id) else {
            return 0.0;
        };

        let start = current_minute - self.window_minutes;
        let mut total = 0_u64;
        for minute in start..current_minute {
            total += u64::from(buckets.get(&minute).copied().unwrap_or(0));
        }

        if self.window_minutes <= 0 {
            return 0.0;
        }
        total as f64 / self.window_minutes as f64
    }
}

fn minute_bucket(ts_unix_ms: i64) -> i64 {
    ts_unix_ms / 60_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_rate_then_burst_flags_spike() {
        let mut tracker = RateTracker::default();
        let agent = "agent/oncall";
        let base_ms = 1_700_000_000_000_i64;
        let current_minute = minute_bucket(base_ms + 60 * 60_000);

        for offset in 0..60 {
            let minute = current_minute - 60 + offset;
            tracker.record(agent, minute * 60_000);
        }

        assert_eq!(tracker.exchange_rate_per_min(agent, current_minute * 60_000), 0);
        for _ in 0..200 {
            tracker.record(agent, current_minute * 60_000 + 1);
        }

        assert_eq!(tracker.exchange_rate_per_min(agent, current_minute * 60_000 + 1), 200);
        assert!(tracker.is_spike(agent, current_minute * 60_000 + 1));
    }
}
