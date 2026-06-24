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
