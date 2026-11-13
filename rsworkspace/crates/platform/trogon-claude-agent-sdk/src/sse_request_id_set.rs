//! A bounded set of request ids.

use indexmap::IndexSet;

use crate::{SseLogLevel, SseLogRecord};

/// Request ids in insertion order, with a cap: past it the oldest id is evicted, and the eviction is logged unless
/// the id is accounted for in a companion set.
#[derive(Debug)]
pub(crate) struct SseRequestIdSet {
    name: &'static str,
    cap: usize,
    ids: IndexSet<String>,
}

impl SseRequestIdSet {
    pub(crate) fn new(name: &'static str, cap: usize) -> Self {
        Self {
            name,
            cap,
            ids: IndexSet::new(),
        }
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    pub(crate) fn remove(&mut self, id: &str) {
        self.ids.shift_remove(id);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> {
        self.ids.iter().map(String::as_str)
    }

    /// Adds `id` (a repeat keeps its place) and evicts the oldest id when the set grew past its cap. The eviction
    /// is logged when the evicted id is not in `shadow`.
    pub(crate) fn insert(&mut self, id: &str, shadow: &Self) -> Option<SseLogRecord> {
        self.ids.insert(id.to_owned());
        if self.ids.len() <= self.cap {
            return None;
        }
        let oldest = self.ids.shift_remove_index(0)?;
        (!shadow.contains(&oldest)).then(|| SseLogRecord {
            level: SseLogLevel::Warn,
            text: format!(
                "[BrowserSSETransport] {} overflow \u{2014} evicting oldest unanswered request_id",
                self.name
            ),
        })
    }
}

#[cfg(test)]
#[path = "sse_request_id_set_tests.rs"]
mod tests;
