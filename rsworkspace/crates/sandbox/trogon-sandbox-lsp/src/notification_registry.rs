use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::lock_ext::LockExt;
use crate::notification_handler::NotificationHandler;

struct HandlerEntry {
    id: u64,
    handler: NotificationHandler,
}

/// The notification listeners of one connection, by method.
#[derive(Default)]
pub(crate) struct NotificationRegistry {
    next_id: AtomicU64,
    entries: Mutex<HashMap<String, Vec<HandlerEntry>>>,
}

impl NotificationRegistry {
    pub(crate) fn add(&self, method: &str, handler: NotificationHandler) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.entries
            .locked()
            .entry(method.to_owned())
            .or_default()
            .push(HandlerEntry { id, handler });
        id
    }

    pub(crate) fn remove(&self, method: &str, id: u64) {
        self.entries
            .locked()
            .get_mut(method)
            .into_iter()
            .for_each(|entries| entries.retain(|entry| entry.id != id));
    }

    /// Calls every listener of `method`. A panicking listener must not break the output pump.
    pub(crate) fn dispatch(&self, method: &str, params: &Value) {
        let handlers: Vec<NotificationHandler> = self
            .entries
            .locked()
            .get(method)
            .map(|entries| entries.iter().map(|e| e.handler.clone()).collect())
            .unwrap_or_default();
        for handler in handlers {
            let _ = catch_unwind(AssertUnwindSafe(|| handler(params)));
        }
    }
}
