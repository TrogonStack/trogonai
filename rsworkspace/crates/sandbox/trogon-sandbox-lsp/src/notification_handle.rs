use std::sync::Arc;

use crate::notification_registry::NotificationRegistry;

/// Removes a notification listener when [`NotificationHandle::detach`] is called. Dropping the
/// handle leaves the listener attached.
pub struct NotificationHandle {
    registry: Arc<NotificationRegistry>,
    method: String,
    id: u64,
}

impl NotificationHandle {
    pub(crate) fn new(registry: Arc<NotificationRegistry>, method: &str, id: u64) -> Self {
        Self {
            registry,
            method: method.to_owned(),
            id,
        }
    }

    pub fn detach(self) {
        self.registry.remove(&self.method, self.id);
    }
}
