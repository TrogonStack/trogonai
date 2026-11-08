use std::collections::{HashMap, VecDeque};

use trogonai_proto::platform::audit::v1::AuditClass;

use crate::failure_reason::FailureReason;
use crate::record::Record;
use crate::writer_stats::WriterStats;

/// Everything the writer mutates outside the chain gate, behind one lock that is
/// never held across an await. The buffers hold records not yet numbered, so a
/// record dropped from them leaves no gap in the chain.
#[derive(Debug, Default)]
pub(crate) struct WriterState {
    pub(crate) retry: VecDeque<Record>,
    pub(crate) queue: VecDeque<Record>,
    /// The chain cannot advance: its head failed to publish and waits for a
    /// retry, or its sequence is exhausted.
    pub(crate) blocked: bool,
    /// The retry buffer filled and has not emptied since.
    pub(crate) overflowed: bool,
    pub(crate) dropped: HashMap<(AuditClass, FailureReason), u64>,
    pub(crate) refused: HashMap<FailureReason, u64>,
    pub(crate) possible_duplicates: u64,
}

impl WriterState {
    pub(crate) fn degraded(&self) -> bool {
        self.blocked || self.overflowed
    }

    pub(crate) fn count_drop(&mut self, class: AuditClass, reason: FailureReason) {
        *self.dropped.entry((class, reason)).or_default() += 1;
    }

    pub(crate) fn count_refusal(&mut self, reason: FailureReason) {
        *self.refused.entry(reason).or_default() += 1;
    }

    pub(crate) fn stats(&self) -> WriterStats {
        WriterStats {
            dropped: self.dropped.clone(),
            refused: self.refused.clone(),
            degraded: self.degraded(),
            retry_buffered: self.retry.len(),
            best_effort_queued: self.queue.len(),
            possible_duplicates: self.possible_duplicates,
        }
    }
}
