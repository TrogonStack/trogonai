use std::{
    error::Error,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use trogon_claude_agent_sdk::{
    InMemorySessionStore, SessionEntry, SessionId, SessionKey, SessionProjectKey, SessionStore, SessionStoreError,
    SessionStoreFuture, SessionStoreOperation, SessionStoreOptions, SessionSubpath, adapters::WallClock,
};

type TestResult = Result<(), Box<dyn Error>>;

fn key(project: &str, session: &str) -> Result<SessionKey, Box<dyn Error>> {
    Ok(SessionKey::new(
        SessionProjectKey::new(project)?,
        SessionId::from(session),
    ))
}

fn entry(text: &str) -> Result<SessionEntry, Box<dyn Error>> {
    Ok(SessionEntry::try_from(serde_json::from_str::<serde_json::Value>(
        text,
    )?)?)
}

#[derive(Clone)]
struct ScriptedClock {
    millis: u64,
    panic_on_read: Arc<AtomicBool>,
}

impl ScriptedClock {
    fn at(millis: u64) -> Self {
        Self {
            millis,
            panic_on_read: Arc::default(),
        }
    }
}

impl WallClock for ScriptedClock {
    type Time = SystemTime;

    fn now(&self) -> Self::Time {
        assert!(!self.panic_on_read.load(Ordering::SeqCst), "scripted clock failure");
        UNIX_EPOCH + Duration::from_millis(self.millis)
    }
}

struct AppendOnlyStore;

impl SessionStore for AppendOnlyStore {
    fn append<'a>(&'a self, _key: &'a SessionKey, _entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn load<'a>(&'a self, _key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test]
async fn append_keeps_every_entry_like_upstream_and_load_returns_none_for_unknown_keys() -> TestResult {
    let store = InMemorySessionStore::new();
    let key = key("project", "session")?;
    assert_eq!(store.load(&key).await?, None);
    let first = entry(r#"{"type":"user","uuid":"u-1"}"#)?;
    let anonymous = entry(r#"{"type":"user"}"#)?;
    store
        .append(&key, &[first.clone(), first.clone(), anonymous.clone()])
        .await?;
    store.append(&key, &[first.clone(), anonymous.clone()]).await?;
    assert_eq!(
        store.load(&key).await?,
        Some(vec![
            first.clone(),
            first.clone(),
            anonymous.clone(),
            first,
            anonymous.clone()
        ])
    );
    assert_eq!(format!("{store:?}"), "InMemorySessionStore { .. }");
    Ok(())
}

#[tokio::test]
async fn sessions_list_newest_first_and_exclude_subpaths_and_other_projects() -> TestResult {
    let store = InMemorySessionStore::new();
    let older = key("project", "older")?;
    let newer = key("project", "newer")?;
    let other = key("other", "elsewhere")?;
    let nested = newer.clone().with_subpath(SessionSubpath::new("subagents/agent-1"));
    for target in [&older, &newer, &other, &nested] {
        store.append(target, &[entry(r#"{"type":"user"}"#)?]).await?;
    }
    let sessions = store.list_sessions(older.project_key()).await?;
    let ids: Vec<_> = sessions.iter().map(|session| session.session_id.as_str()).collect();
    assert_eq!(ids, ["newer", "older"]);
    assert!(sessions[0].modified_at_ms > sessions[1].modified_at_ms);
    assert_eq!(store.list_sessions(other.project_key()).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn deleting_a_main_key_cascades_but_deleting_a_subpath_is_exact() -> TestResult {
    let store = InMemorySessionStore::new();
    let main = key("project", "session")?;
    let first = main.clone().with_subpath(SessionSubpath::new("subagents/a"));
    let second = main.clone().with_subpath(SessionSubpath::new("subagents/b"));
    let unrelated = key("project", "unrelated")?;
    for target in [&main, &first, &second, &unrelated] {
        store.append(target, &[entry(r#"{"type":"user"}"#)?]).await?;
    }
    assert_eq!(store.list_subkeys(&main).await?, ["subagents/a", "subagents/b"]);

    store.delete(&first).await?;
    assert_eq!(store.list_subkeys(&main).await?, ["subagents/b"]);
    assert!(store.load(&main).await?.is_some());

    store.delete(&main).await?;
    assert_eq!(store.load(&main).await?, None);
    assert_eq!(store.load(&second).await?, None);
    assert!(store.list_subkeys(&main).await?.is_empty());
    assert!(store.load(&unrelated).await?.is_some());
    store.delete(&main).await?;
    Ok(())
}

#[tokio::test]
async fn stores_without_optional_operations_report_the_documented_defaults() -> TestResult {
    let store = AppendOnlyStore;
    let key = key("project", "session")?;
    store.append(&key, &[]).await?;
    assert_eq!(store.load(&key).await?, None);
    let list = store
        .list_sessions(key.project_key())
        .await
        .err()
        .ok_or("list succeeded")?;
    assert!(matches!(
        list,
        SessionStoreError::Unsupported(SessionStoreOperation::List)
    ));
    let delete = store.delete(&key).await.err().ok_or("delete succeeded")?;
    assert!(matches!(
        delete,
        SessionStoreError::Unsupported(SessionStoreOperation::Delete)
    ));
    assert!(store.list_subkeys(&key).await?.is_empty());
    Ok(())
}

#[test]
fn operations_and_errors_describe_themselves() {
    for (operation, text) in [
        (SessionStoreOperation::Append, "append"),
        (SessionStoreOperation::Load, "load"),
        (SessionStoreOperation::List, "list"),
        (SessionStoreOperation::Delete, "delete"),
        (SessionStoreOperation::ListSubkeys, "list subkeys"),
    ] {
        assert_eq!(operation.to_string(), text);
        assert_eq!(
            SessionStoreError::Unsupported(operation).to_string(),
            format!("session store operation is unsupported: {text}")
        );
    }
    let backend = SessionStoreError::backend(std::io::Error::other("disk"));
    assert_eq!(backend.to_string(), "session store backend failed");
    assert_eq!(
        std::error::Error::source(&backend).map(ToString::to_string).as_deref(),
        Some("disk")
    );
    assert_eq!(format!("{backend:?}"), "SessionStoreError { .. }");
    assert_eq!(
        SessionStoreError::Poisoned.to_string(),
        "session store lock is poisoned"
    );
    assert_eq!(
        SessionStoreError::ClockExhausted.to_string(),
        "session store clock exhausted"
    );
}

#[test]
fn options_wrap_shared_stores_with_default_limits() {
    let concrete = SessionStoreOptions::from(Arc::new(InMemorySessionStore::new()));
    let erased = SessionStoreOptions::from(Arc::new(AppendOnlyStore) as Arc<dyn SessionStore>);
    for options in [concrete, erased] {
        assert!(options.config_directory.is_none());
        assert!(!options.require_absolute_config_paths);
        assert!(options.project_key.is_none());
        assert_eq!(
            options.limits.max_pending_entries,
            NonZeroUsize::MIN.saturating_add(499)
        );
        assert_eq!(options.limits.max_pending_bytes.get(), 1024 * 1024);
        assert_eq!(options.limits.max_resume_entries, None);
        assert_eq!(options.limits.max_resume_bytes, None);
        assert_eq!(options.limits.max_subkeys, None);
        assert!(format!("{options:?}").starts_with("SessionStoreOptions { limits: SessionStoreLimits"));
    }
}

#[tokio::test]
async fn an_exhausted_modification_clock_is_reported() -> TestResult {
    let store = InMemorySessionStore::with_clock(ScriptedClock::at(u64::MAX));
    let key = key("project", "session")?;
    store.append(&key, &[entry(r#"{"type":"user"}"#)?]).await?;
    let error = store
        .append(&key, &[entry(r#"{"type":"user"}"#)?])
        .await
        .err()
        .ok_or("second append succeeded")?;
    assert!(matches!(error, SessionStoreError::ClockExhausted));
    Ok(())
}

#[tokio::test]
async fn a_poisoned_store_lock_is_reported_by_every_operation() -> TestResult {
    let clock = ScriptedClock::at(1);
    let store = InMemorySessionStore::with_clock(clock.clone());
    let key = key("project", "session")?;
    clock.panic_on_read.store(true, Ordering::SeqCst);
    let poisoner = store.clone();
    let poison_key = key.clone();
    let poison_entry = entry(r#"{"type":"user"}"#)?;
    let joined = tokio::spawn(async move {
        let _ = poisoner.append(&poison_key, &[poison_entry]).await;
    })
    .await;
    assert!(joined.is_err());
    clock.panic_on_read.store(false, Ordering::SeqCst);

    assert!(matches!(
        store.append(&key, &[entry(r#"{"type":"user"}"#)?]).await,
        Err(SessionStoreError::Poisoned)
    ));
    assert!(matches!(store.load(&key).await, Err(SessionStoreError::Poisoned)));
    assert!(matches!(
        store.list_sessions(key.project_key()).await,
        Err(SessionStoreError::Poisoned)
    ));
    assert!(matches!(store.delete(&key).await, Err(SessionStoreError::Poisoned)));
    assert!(matches!(
        store.list_subkeys(&key).await,
        Err(SessionStoreError::Poisoned)
    ));
    Ok(())
}
