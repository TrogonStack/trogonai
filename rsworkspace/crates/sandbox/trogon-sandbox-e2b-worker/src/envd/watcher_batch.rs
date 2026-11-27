use serde_json::Value;

use super::WatcherEvent;
use super::constants::MAX_WATCHER_EVENTS_PER_FETCH;

/// What one `GetWatcherEvents` call returned.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct WatcherBatch {
    pub events: Vec<WatcherEvent>,
    /// Events past the per-fetch ceiling that were discarded.
    pub dropped: usize,
}

impl WatcherBatch {
    pub(super) fn from_reply(reply: &Value) -> Self {
        let mut batch = Self::default();
        let events = reply
            .get("events")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for event in events.iter().filter_map(WatcherEvent::from_value) {
            if batch.events.len() >= MAX_WATCHER_EVENTS_PER_FETCH {
                batch.dropped += 1;
            } else {
                batch.events.push(event);
            }
        }
        batch
    }
}
