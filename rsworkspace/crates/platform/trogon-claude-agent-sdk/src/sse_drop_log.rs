//! Drop counting and log sampling.

use indexmap::IndexMap;

use crate::{SseDropCategory, SseLogLevel, SseLogRecord};

const LOGGED_IN_FULL: u64 = 5;
const SAMPLE_EVERY: u64 = 50;

/// Counts drops per category and decides which ones are worth a log line: the first five of each category, then
/// every fiftieth, which carries the running total.
#[derive(Debug, Default)]
pub(crate) struct SseDropLog {
    counts: IndexMap<SseDropCategory, u64>,
}

impl SseDropLog {
    /// Counts one drop and returns the line to log for it, if any.
    pub(crate) fn record(
        &mut self,
        category: SseDropCategory,
        level: SseLogLevel,
        text: String,
    ) -> Option<SseLogRecord> {
        let count = self.counts.entry(category).or_insert(0);
        *count += 1;
        let text = if *count <= LOGGED_IN_FULL {
            text
        } else if *count % SAMPLE_EVERY == 0 {
            format!("{text} ({count} total \u{2014} sampling 1/{SAMPLE_EVERY})")
        } else {
            return None;
        };
        Some(SseLogRecord { level, text })
    }

    /// The drop counts, in the order the categories first occurred.
    pub(crate) fn counts(&self) -> Vec<(SseDropCategory, u64)> {
        self.counts
            .iter()
            .map(|(category, count)| (*category, *count))
            .collect()
    }
}

#[cfg(test)]
#[path = "sse_drop_log_tests.rs"]
mod tests;
