use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::push_notification_config_id::PushNotificationConfigId;
use crate::task_id::A2aTaskId;

type RegistryMap = HashMap<(A2aTaskId, PushNotificationConfigId), DeliverySemantics>;

/// Bridge-held extension for `tasks/pushNotificationConfig/*` payloads until upstream `TaskPushNotificationConfig` carries semantics.
#[derive(Clone, Default)]
pub struct PushDeliverySemanticsRegistry {
    pairs: Arc<Mutex<RegistryMap>>,
}

/// Recover from a poisoned mutex by reading through the poison — the map only
/// stores plain data, so a partial write from a panicking thread can't leave
/// the structure in a logically corrupt state. Re-locking would just panic
/// every reader after a single unrelated panic.
fn through_poison<'a, T>(result: Result<MutexGuard<'a, T>, PoisonError<MutexGuard<'a, T>>>) -> MutexGuard<'a, T> {
    match result {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl PushDeliverySemanticsRegistry {
    pub fn set(&self, task_id: A2aTaskId, cfg_id: PushNotificationConfigId, semantics: DeliverySemantics) {
        through_poison(self.pairs.lock()).insert((task_id, cfg_id), semantics);
    }

    pub fn remove(&self, task_id: &A2aTaskId, cfg_id: &PushNotificationConfigId) {
        through_poison(self.pairs.lock()).remove(&(task_id.clone(), cfg_id.clone()));
    }

    pub fn get(&self, task_id: &A2aTaskId, cfg_id: &PushNotificationConfigId) -> DeliverySemantics {
        through_poison(self.pairs.lock())
            .get(&(task_id.clone(), cfg_id.clone()))
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
