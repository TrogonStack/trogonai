use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::audit_sink::AuditSink;
use crate::sealed_record::SealedRecord;
use crate::sink_error::SinkError;

/// An in-memory sink with failure injection, for testing how a surface behaves
/// when audit is down, full or slow to acknowledge.
///
/// Like the stream it stands in for, it keeps one copy of each record id, so a
/// retry under the same id is a duplicate and not a second record.
#[derive(Debug, Clone, Default)]
pub struct MemorySink {
    state: Arc<Mutex<MemoryState>>,
}

#[derive(Debug, Default)]
struct MemoryState {
    stored: Vec<SealedRecord>,
    attempts: usize,
    failures: VecDeque<SinkError>,
    lost_acks: usize,
    failing: Option<SinkError>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every record stored, in order, each id once.
    pub fn stored(&self) -> Vec<SealedRecord> {
        self.state().stored.clone()
    }

    /// How many publishes were attempted, stored or not.
    pub fn attempts(&self) -> usize {
        self.state().attempts
    }

    /// The next `count` publishes fail with `error` and store nothing.
    pub fn fail_next(&self, count: usize, error: SinkError) {
        self.state().failures.extend(std::iter::repeat_n(error, count));
    }

    /// Every publish fails with `error` until [`MemorySink::recover`].
    pub fn fail_all(&self, error: SinkError) {
        self.state().failing = Some(error);
    }

    /// The next `count` publishes store the record but report a timeout, as a
    /// lost acknowledgement does.
    pub fn lose_next_acks(&self, count: usize) {
        self.state().lost_acks = count;
    }

    pub fn recover(&self) {
        let mut state = self.state();
        state.failing = None;
        state.failures.clear();
        state.lost_acks = 0;
    }
}

impl AuditSink for MemorySink {
    async fn publish(&self, record: &SealedRecord) -> Result<(), SinkError> {
        let mut state = self.state();
        state.attempts += 1;
        if let Some(error) = state.failing {
            return Err(error);
        }
        if let Some(error) = state.failures.pop_front() {
            return Err(error);
        }
        if !state.stored.iter().any(|stored| stored.id() == record.id()) {
            state.stored.push(record.clone());
        }
        if state.lost_acks > 0 {
            state.lost_acks -= 1;
            return Err(SinkError::Timeout);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
