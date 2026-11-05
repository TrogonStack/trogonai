use std::collections::HashMap;

use trogonai_proto::platform::audit::v1::AuditClass;

use crate::failure_reason::FailureReason;

/// A point-in-time copy of the writer's counters and gauges.
///
/// OpenTelemetry wiring is a later slice and reads this: the drop counter by class
/// and reason is what an operator reads beside the `would_deny` totals, and a
/// `Must` drop is the counter that pages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriterStats {
    pub(crate) dropped: HashMap<(AuditClass, FailureReason), u64>,
    pub(crate) refused: HashMap<FailureReason, u64>,
    pub(crate) degraded: bool,
    pub(crate) retry_buffered: usize,
    pub(crate) best_effort_queued: usize,
}

impl WriterStats {
    pub fn dropped(&self, class: AuditClass, reason: FailureReason) -> u64 {
        self.dropped.get(&(class, reason)).copied().unwrap_or(0)
    }

    pub fn dropped_total(&self, class: AuditClass) -> u64 {
        self.dropped
            .iter()
            .filter(|((dropped_class, _), _)| *dropped_class == class)
            .map(|(_, count)| count)
            .sum()
    }

    /// Actions refused because their must-audit record could not be written.
    pub fn refused(&self, reason: FailureReason) -> u64 {
        self.refused.get(&reason).copied().unwrap_or(0)
    }

    pub fn refused_total(&self) -> u64 {
        self.refused.values().sum()
    }

    /// True while the retry buffer has been full and has not yet drained. A
    /// surface fails readiness on it.
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    pub fn retry_buffered(&self) -> usize {
        self.retry_buffered
    }

    pub fn best_effort_queued(&self) -> usize {
        self.best_effort_queued
    }
}

#[cfg(test)]
mod tests;
