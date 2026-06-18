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
mod tests {
    use super::*;
    use crate::push::idempotency_key_header::IdempotencyKeyHeader;

    fn task() -> A2aTaskId {
        A2aTaskId::new("task-1").unwrap()
    }

    fn cfg() -> PushNotificationConfigId {
        PushNotificationConfigId::new("cfg-1").unwrap()
    }

    #[test]
    fn get_returns_at_least_once_default_when_unset() {
        let registry = PushDeliverySemanticsRegistry::default();
        assert_eq!(registry.get(&task(), &cfg()), DeliverySemantics::AtLeastOnce);
    }

    #[test]
    fn set_then_get_round_trips_exactly_once_with_header() {
        let registry = PushDeliverySemanticsRegistry::default();
        let semantics = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: Some(IdempotencyKeyHeader::try_from("X-Push-Key").unwrap()),
        };
        registry.set(task(), cfg(), semantics.clone());
        assert_eq!(registry.get(&task(), &cfg()), semantics);
    }

    #[test]
    fn remove_drops_stored_semantics_back_to_default() {
        let registry = PushDeliverySemanticsRegistry::default();
        registry.set(
            task(),
            cfg(),
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
        );
        registry.remove(&task(), &cfg());
        assert_eq!(registry.get(&task(), &cfg()), DeliverySemantics::AtLeastOnce);
    }

    #[test]
    fn through_poison_recovers_lock_after_panic() {
        use std::sync::Mutex;
        let lock: Mutex<Vec<i32>> = Mutex::new(Vec::new());
        let lock_ref = std::sync::Arc::new(lock);
        let poisoner = std::sync::Arc::clone(&lock_ref);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        let recovered = through_poison(lock_ref.lock());
        assert!(recovered.is_empty());
    }
}
