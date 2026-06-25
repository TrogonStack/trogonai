use std::sync::atomic::{AtomicI64, Ordering};

use super::*;

#[test]
fn default_constructor_matches_new() {
    let _: InMemoryReplayStore = InMemoryReplayStore::default();
}

#[tokio::test(flavor = "current_thread")]
async fn first_insert_then_replay_fails() {
    let store = InMemoryReplayStore::new();
    assert!(store.check_and_insert("k1", 60).await.unwrap());
    assert!(!store.check_and_insert("k1", 60).await.unwrap());
}

#[tokio::test(flavor = "current_thread")]
async fn poisoned_mutex_yields_typed_error() {
    // Regression: previously the error path stringified PoisonError into
    // Backend(String). Verify that a poisoned mutex now lands in the
    // dedicated MutexPoisoned variant — callers can match on it.
    let store = std::sync::Arc::new(InMemoryReplayStore::new());
    let s2 = std::sync::Arc::clone(&store);
    let _ = std::thread::spawn(move || {
        let _guard = s2.inner.lock().expect("acquire");
        panic!("intentional poison");
    })
    .join();
    let err = store.check_and_insert("k", 60).await.unwrap_err();
    assert!(matches!(err, ReplayError::MutexPoisoned), "got {err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn expires_after_ttl() {
    let now = std::sync::Arc::new(AtomicI64::new(1000));
    let now_c = now.clone();
    let store = InMemoryReplayStore::new().with_clock(move || now_c.load(Ordering::SeqCst));
    assert!(store.check_and_insert("k", 5).await.unwrap());
    now.store(1010, Ordering::SeqCst);
    assert!(store.check_and_insert("k", 5).await.unwrap());
}
