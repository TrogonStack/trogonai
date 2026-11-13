#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Clock injection tests use deterministic session entry fixtures."
    )
)]

use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use trogon_claude_agent_sdk::{
    InMemorySessionStore, SessionEntry, SessionId, SessionKey, SessionProjectKey, SessionStore, adapters::WallClock,
};

#[derive(Clone)]
struct SequenceClock {
    times: Arc<[SystemTime]>,
    calls: Arc<AtomicUsize>,
}

impl WallClock for SequenceClock {
    type Time = SystemTime;

    fn now(&self) -> Self::Time {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        self.times.get(index).copied().unwrap_or(UNIX_EPOCH)
    }
}

fn key() -> Result<SessionKey, Box<dyn std::error::Error>> {
    Ok(SessionKey::new(
        SessionProjectKey::new("clock-project")?,
        SessionId::from("clock-session"),
    )?)
}

fn entry(value: usize) -> Result<SessionEntry, Box<dyn std::error::Error>> {
    Ok(SessionEntry::new(serde_json::from_value(
        json!({"type":"assistant","value":value}),
    )?))
}

#[tokio::test]
async fn injected_wall_clock_sets_exact_session_timestamp() -> Result<(), Box<dyn std::error::Error>> {
    let calls = Arc::new(AtomicUsize::new(0));
    let clock = SequenceClock {
        times: Arc::from([UNIX_EPOCH + Duration::from_millis(1234)]),
        calls: calls.clone(),
    };
    let store = InMemorySessionStore::with_clock(clock);
    let key = key()?;
    store.append(&key, &[]).await?;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(store.list_sessions(key.project_key()).await?.is_empty());
    store.append(&key, &[entry(1)?]).await?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let sessions = store.list_sessions(key.project_key()).await?;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].modified_at_ms, 1234);
    Ok(())
}

#[tokio::test]
async fn repeated_and_backward_wall_time_keeps_session_timestamps_strictly_increasing()
-> Result<(), Box<dyn std::error::Error>> {
    let calls = Arc::new(AtomicUsize::new(0));
    let clock = SequenceClock {
        times: Arc::from([
            UNIX_EPOCH + Duration::from_millis(1000),
            UNIX_EPOCH + Duration::from_millis(1000),
            UNIX_EPOCH + Duration::from_millis(900),
            UNIX_EPOCH - Duration::from_millis(1),
            UNIX_EPOCH + Duration::from_millis(5000),
        ]),
        calls: calls.clone(),
    };
    let store = InMemorySessionStore::with_clock(clock);
    let key = key()?;
    for (index, expected) in [1000, 1001, 1002, 1003, 5000].into_iter().enumerate() {
        store.append(&key, &[entry(index)?]).await?;
        assert_eq!(calls.load(Ordering::SeqCst), index + 1);
        let sessions = store.list_sessions(key.project_key()).await?;
        assert_eq!(sessions[0].modified_at_ms, expected);
    }
    Ok(())
}
