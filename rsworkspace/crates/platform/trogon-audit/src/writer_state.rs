use std::collections::{HashMap, VecDeque};

use trogonai_proto::platform::audit::v1::AuditClass;

use crate::failure_reason::FailureReason;
use crate::sealed_record::SealedRecord;
use crate::writer_stats::WriterStats;

/// Everything the writer mutates, behind one lock that is never held across an
/// await.
#[derive(Debug, Default)]
pub(crate) struct WriterState {
    pub(crate) retry: VecDeque<SealedRecord>,
    pub(crate) queue: VecDeque<SealedRecord>,
    pub(crate) degraded: bool,
    pub(crate) dropped: HashMap<(AuditClass, FailureReason), u64>,
    pub(crate) refused: HashMap<FailureReason, u64>,
}

impl WriterState {
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
            degraded: self.degraded,
            retry_buffered: self.retry.len(),
            best_effort_queued: self.queue.len(),
        }
    }
}
