use async_nats::jetstream::kv;
use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

use super::*;
use crate::{KillReason, OccurredAt};

fn agent(id: &str) -> AgentId {
    AgentId::parse(id).unwrap()
}

fn killed_state() -> KillSwitchState {
    KillSwitchState::Killed {
        reason: KillReason::Manual,
        since: OccurredAt::now(),
    }
}

#[tokio::test]
async fn put_creates_when_entry_absent() {
    let kv = MockJetStreamKvStore::new();
    let store = KillSwitchProjectionStore::new(kv.clone());
    store.put(&agent("agent-1"), &killed_state()).await.unwrap();
    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "agent-1");
}

#[tokio::test]
async fn put_updates_when_entry_present() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 3, kv::Operation::Put);
    let store = KillSwitchProjectionStore::new(kv.clone());
    store.put(&agent("agent-1"), &KillSwitchState::Alive).await.unwrap();
    let updates = kv.update_calls();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, "agent-1");
    assert_eq!(updates[0].2, 3);
}

#[tokio::test]
async fn put_creates_when_latest_entry_is_a_delete_tombstone() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(bytes::Bytes::new(), 7, kv::Operation::Delete);
    let store = KillSwitchProjectionStore::new(kv.clone());
    store.put(&agent("agent-1"), &killed_state()).await.unwrap();
    assert_eq!(kv.update_calls().len(), 0, "must not update against a delete tombstone");
    assert_eq!(kv.create_calls().len(), 1);
}

#[tokio::test]
async fn put_creates_when_latest_entry_is_a_purge_tombstone() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(bytes::Bytes::new(), 9, kv::Operation::Purge);
    let store = KillSwitchProjectionStore::new(kv.clone());
    store.put(&agent("agent-1"), &killed_state()).await.unwrap();
    assert_eq!(kv.update_calls().len(), 0);
    assert_eq!(kv.create_calls().len(), 1);
}

#[tokio::test]
async fn put_retries_when_concurrent_writer_races_create() {
    let kv_store = MockJetStreamKvStore::new();
    kv_store.enqueue_entry_none();
    kv_store.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 5, kv::Operation::Put);
    kv_store.enqueue_update_result(Ok(6));

    let store = KillSwitchProjectionStore::new(kv_store.clone());
    store.put(&agent("agent-1"), &killed_state()).await.unwrap();
    assert_eq!(kv_store.create_calls().len(), 1);
    assert_eq!(kv_store.update_calls().len(), 1);
}

#[tokio::test]
async fn put_retries_when_concurrent_writer_races_update() {
    let kv_store = MockJetStreamKvStore::new();
    kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 3, kv::Operation::Put);
    kv_store.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 4, kv::Operation::Put);
    kv_store.enqueue_update_result(Ok(5));

    let store = KillSwitchProjectionStore::new(kv_store.clone());
    store.put(&agent("agent-1"), &killed_state()).await.unwrap();
    assert_eq!(kv_store.update_calls().len(), 2);
    assert_eq!(kv_store.update_calls()[1].2, 4);
}

#[tokio::test]
async fn put_propagates_non_conflict_update_error_without_retry() {
    let kv_store = MockJetStreamKvStore::new();
    kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 3, kv::Operation::Put);
    kv_store.enqueue_update_result(Err(kv::UpdateErrorKind::TimedOut));
    let store = KillSwitchProjectionStore::new(kv_store.clone());
    let err = store.put(&agent("agent-1"), &killed_state()).await.unwrap_err();
    assert!(matches!(err, KillSwitchProjectionError::Kv(_)));
    assert_eq!(kv_store.update_calls().len(), 1, "fatal update errors must not retry");
}

#[tokio::test]
async fn put_propagates_non_conflict_create_error_without_retry() {
    let kv_store = MockJetStreamKvStore::new();
    kv_store.enqueue_create_result(Err(kv::CreateErrorKind::InvalidKey));
    let store = KillSwitchProjectionStore::new(kv_store.clone());
    let err = store.put(&agent("agent-1"), &killed_state()).await.unwrap_err();
    assert!(matches!(err, KillSwitchProjectionError::Kv(_)));
    assert_eq!(kv_store.create_calls().len(), 1, "fatal create errors must not retry");
}

#[tokio::test]
async fn put_gives_up_when_revision_race_persists_past_retry_budget() {
    let kv_store = MockJetStreamKvStore::new();
    for _ in 0..3 {
        kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 1, kv::Operation::Put);
        kv_store.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    }

    let store = KillSwitchProjectionStore::new(kv_store.clone());
    let err = store.put(&agent("agent-1"), &killed_state()).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "kill switch projection write lost a revision race after 3 attempts"
    );
    assert_eq!(kv_store.update_calls().len(), 3);
}

#[tokio::test]
async fn get_returns_none_when_absent() {
    let kv = MockJetStreamKvStore::new();
    let store = KillSwitchProjectionStore::new(kv);
    let result = store.get(&agent("missing")).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn get_returns_projected_state_when_present() {
    let kv = MockJetStreamKvStore::new();
    let state = killed_state();
    let bytes = encode_kill_status(&state).unwrap();
    kv.enqueue_get_some(bytes::Bytes::from(bytes));
    let store = KillSwitchProjectionStore::new(kv);
    let result = store.get(&agent("agent-1")).await.unwrap();
    assert_eq!(result, Some(state));
}

#[tokio::test]
async fn get_propagates_malformed_projected_value() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_get_some(bytes::Bytes::from_static(b"not json"));
    let store = KillSwitchProjectionStore::new(kv);
    let err = store.get(&agent("agent-1")).await.unwrap_err();
    assert!(matches!(err, KillSwitchProjectionError::Wire(_)));
}

#[tokio::test]
async fn get_propagates_kv_error() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_get_error(kv::EntryErrorKind::TimedOut);
    let store = KillSwitchProjectionStore::new(kv);
    let err = store.get(&agent("agent-1")).await.unwrap_err();
    assert!(matches!(err, KillSwitchProjectionError::Kv(_)));
}

#[tokio::test]
async fn put_round_trips_through_get() {
    // Uses the mock KV store as a stand-in for a real bucket: entry() and
    // get() are independently enqueued in this mock, so this exercises the
    // encode/decode boundary rather than a real put-then-get read of your own
    // write. Full read-after-write behavior is covered by the reference
    // pattern's own JetStream integration surface, not re-proven here.
    let kv = MockJetStreamKvStore::new();
    let state = KillSwitchState::Alive;
    let bytes = encode_kill_status(&state).unwrap();
    kv.enqueue_get_some(bytes::Bytes::from(bytes));

    let store = KillSwitchProjectionStore::new(kv);
    store.put(&agent("agent-1"), &state).await.unwrap();
    let projected = store.get(&agent("agent-1")).await.unwrap();
    assert_eq!(projected, Some(state));
}
