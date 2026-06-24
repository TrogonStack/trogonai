use std::time::Duration;

use agent_client_protocol::{PromptResponse, SessionId, StopReason};
use trogon_std::time::{GetNow, MockClock, MockInstant};

use super::*;

#[test]
fn lock_poisoned_error_display() {
    let err = LockPoisonedError;
    assert_eq!(err.to_string(), "mutex lock poisoned");
}

#[test]
fn lock_poisoned_error_is_std_error() {
    let err: &dyn std::error::Error = &LockPoisonedError;
    assert!(err.source().is_none());
}

#[test]
fn lock_poisoned_error_from_poison_error() {
    use std::sync::{Arc, Mutex};

    let mutex = Arc::new(Mutex::new(42));
    let mutex_clone = mutex.clone();

    // Poison the mutex by panicking in another thread while holding the lock
    let handle = std::thread::spawn(move || {
        let _guard = mutex_clone.lock().expect("test lock");
        panic!("intentional panic to poison mutex");
    });
    let _ = handle.join();

    // Now the mutex is poisoned — verify From<PoisonError> works
    let err = mutex.lock().unwrap_err();
    let _: LockPoisonedError = err.into();
}

#[test]
fn resolve_waiter_returns_false_when_no_waiter_registered() {
    let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
    let resolved = waiters
        .resolve_waiter(
            &SessionId::from("s1"),
            PromptToken(0),
            Ok(PromptResponse::new(StopReason::EndTurn)),
        )
        .unwrap();
    assert!(!resolved);
}

#[test]
fn register_waiter_rejects_duplicate_session() {
    let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
    let session_id = SessionId::from("s1");
    let (_rx, _token) = waiters.register_waiter(session_id.clone()).unwrap();
    assert!(waiters.register_waiter(session_id).is_err());
}

#[test]
fn purge_expired_timed_out_waiters_removes_expired_markers() {
    let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
    let clock = MockClock::new();
    {
        let mut timed_out = waiters.timed_out.lock().unwrap();
        timed_out.insert((SessionId::from("s1"), PromptToken(0)), clock.now());
    }
    assert_eq!(waiters.timed_out.lock().unwrap().len(), 1);

    clock.advance(PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW + Duration::from_millis(1));
    waiters.purge_expired_timed_out_waiters(&clock).unwrap();

    assert!(waiters.timed_out.lock().unwrap().is_empty());
}

#[test]
fn purge_keeps_non_expired_markers() {
    let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
    let clock = MockClock::new();
    let old_instant = clock.now();
    clock.advance(PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW + Duration::from_millis(1));
    let fresh_instant = clock.now();
    {
        let mut timed_out = waiters.timed_out.lock().unwrap();
        timed_out.insert((SessionId::from("old"), PromptToken(0)), old_instant);
        timed_out.insert((SessionId::from("fresh"), PromptToken(1)), fresh_instant);
    }
    assert_eq!(waiters.timed_out.lock().unwrap().len(), 2);

    waiters.purge_expired_timed_out_waiters(&clock).unwrap();

    let timed_out = waiters.timed_out.lock().unwrap();
    assert_eq!(timed_out.len(), 1);
    assert!(timed_out.contains_key(&(SessionId::from("fresh"), PromptToken(1))));
}
