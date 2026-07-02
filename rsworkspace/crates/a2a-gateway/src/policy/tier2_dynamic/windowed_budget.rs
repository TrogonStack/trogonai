use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use super::budget_metric::BudgetMetric;
use super::window_duration::WindowDuration;
use crate::policy::tier2::rule_name::RuleName;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BudgetKey {
    rule: RuleNameKey,
    metric: BudgetMetric,
}

/// `RuleName` doesn't implement `Copy`, so the map key clones its owned
/// string once per distinct rule rather than on every lookup.
type RuleNameKey = Box<str>;

#[derive(Debug, Clone, Copy)]
struct Bucket {
    epoch: i64,
    consumed: f64,
}

/// Process-local, in-memory epoch-bucketed counter backing the
/// `token_count_per_window` and `cost_per_window` dynamic conditions.
///
/// V1 scope only: counters reset on process restart and are not shared
/// across gateway replicas. A durable, cross-replica implementation
/// (e.g. backed by JetStream KV) is tracked as follow-up work in
/// `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md` (WI-09).
#[derive(Debug, Default)]
pub struct WindowedBudget {
    buckets: Mutex<HashMap<BudgetKey, Bucket>>,
}

impl WindowedBudget {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `amount` against `rule`/`metric`'s current window bucket
    /// (rolling over to a fresh bucket when `now` has moved past the
    /// current bucket's window) and reports whether the post-record total
    /// stays within `limit`.
    ///
    /// Recording happens unconditionally, including on breach, so a
    /// caller that keeps retrying after being denied doesn't get a free
    /// pass once the window rolls over faster than their actual usage
    /// would justify.
    pub fn check_and_record(
        &self,
        rule: &RuleName,
        metric: BudgetMetric,
        window: WindowDuration,
        limit: f64,
        amount: f64,
        now: DateTime<Utc>,
    ) -> bool {
        let key = BudgetKey {
            rule: Box::from(rule.as_str()),
            metric,
        };
        let window_seconds = window.as_seconds().max(1);
        let epoch = now.timestamp().div_euclid(window_seconds);

        let Ok(mut buckets) = self.buckets.lock() else {
            // A poisoned lock means a prior panic corrupted shared state.
            // Fail closed: treat the budget as exhausted rather than risk
            // evaluating against inconsistent counters.
            return false;
        };
        let bucket = buckets.entry(key).or_insert(Bucket { epoch, consumed: 0.0 });
        if bucket.epoch != epoch {
            bucket.epoch = epoch;
            bucket.consumed = 0.0;
        }
        bucket.consumed += amount;
        bucket.consumed <= limit
    }
}
