use std::{
    error::Error,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde_json::json;
use trogon_claude_agent_sdk::{
    Environment, InMemorySessionStore, MirrorEnqueueError, MirrorFailureError, SessionEntry, SessionError, SessionId,
    SessionKey, SessionProjectKey, SessionRuntime, SessionStore, SessionStoreError, SessionStoreFuture,
    SessionStoreOptions, SessionSubpath, StoreResume, SummaryDialect, Timeout, TranscriptMirror,
};

type TestResult = Result<(), Box<dyn Error>>;
const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";

fn memory_mirror_root() -> PathBuf {
    PathBuf::from(if cfg!(windows) {
        r"C:\sdk-memory-mirror"
    } else {
        "/sdk-memory-mirror"
    })
}

fn key(project: &str, session: &str) -> Result<SessionKey, Box<dyn Error>> {
    Ok(SessionKey::new(
        SessionProjectKey::new(project)?,
        SessionId::from(session),
    )?)
}
fn entry(kind: &str, value: usize) -> Result<SessionEntry, Box<dyn Error>> {
    Ok(SessionEntry::try_from(
        json!({"type":kind,"value":value,"nested":{"items":[1,2]}}),
    )?)
}
fn options(store: Arc<dyn SessionStore>, root: &Path) -> Result<SessionStoreOptions, Box<dyn Error>> {
    let mut options = SessionStoreOptions::from(store);
    options.config_directory = Some(root.to_path_buf());
    options.project_key = Some(SessionProjectKey::new("project")?);
    Ok(options)
}

#[tokio::test]
async fn store_preserves_order_deep_values_and_load_isolation() -> TestResult {
    let store = InMemorySessionStore::new();
    let key = key("project", SESSION)?;
    let first = entry("user", 1)?;
    let second = entry("assistant", 2)?;
    store.append(&key, std::slice::from_ref(&first)).await?;
    store.append(&key, std::slice::from_ref(&second)).await?;
    assert_eq!(store.load(&key).await?, Some(vec![first.clone(), second.clone()]));
    let mut loaded = store.load(&key).await?.ok_or("missing transcript")?;
    loaded.clear();
    let mut changed = first.into_object();
    changed.insert("value".into(), 99.into());
    assert_ne!(
        store.load(&key).await?.ok_or("missing transcript")?[0].as_object(),
        &changed
    );
    assert_eq!(store.list_sessions(key.project_key()).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn empty_append_does_not_create_a_session() -> TestResult {
    let store = InMemorySessionStore::new();
    let key = key("project", SESSION)?;
    store.append(&key, &[]).await?;
    assert_eq!(store.load(&key).await?, None);
    assert!(store.list_sessions(key.project_key()).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn repeated_stable_uuids_are_stored_again_like_upstream_reference_stores() -> TestResult {
    let store = InMemorySessionStore::new();
    let key = key("project", SESSION)?;
    let value = SessionEntry::try_from(json!({"type":"user","uuid":"stable","nested":{"items":[1,2]}}))?;
    store.append(&key, std::slice::from_ref(&value)).await?;
    store.append(&key, std::slice::from_ref(&value)).await?;
    assert_eq!(store.load(&key).await?, Some(vec![value.clone(), value]));
    Ok(())
}

#[tokio::test]
async fn projects_sessions_and_subpaths_are_isolated_and_delete_cascades() -> TestResult {
    let store = InMemorySessionStore::new();
    let main = key("project", SESSION)?;
    let sub = main.clone().with_subpath(SessionSubpath::new("subagents/agent-one")?);
    let other = key("other-project", SESSION)?;
    store.append(&main, &[entry("user", 1)?]).await?;
    store.append(&sub, &[entry("assistant", 2)?]).await?;
    store.append(&other, &[entry("user", 3)?]).await?;
    assert_eq!(
        store.list_subkeys(&main).await?,
        vec![String::from("subagents/agent-one")]
    );
    assert_eq!(store.list_sessions(main.project_key()).await?.len(), 1);
    store.delete(&sub).await?;
    assert!(store.load(&main).await?.is_some());
    assert!(store.load(&sub).await?.is_none());
    store.append(&sub, &[entry("assistant", 4)?]).await?;
    store.delete(&main).await?;
    assert!(store.load(&main).await?.is_none());
    assert!(store.load(&sub).await?.is_none());
    assert!(store.load(&other).await?.is_some());
    Ok(())
}

#[test]
fn store_paths_reject_traversal_and_windows_aliases() -> TestResult {
    for value in [
        "",
        "../escape",
        "/absolute",
        "\\absolute",
        "subagents/../escape",
        "subagents\\..\\escape",
        "subagents/./here",
        "C:escape",
        "subagents/nul\0byte",
    ] {
        assert!(SessionSubpath::new(value).is_err(), "accepted unsafe subpath");
    }
    if cfg!(windows) {
        for value in [
            "subagents/NUL",
            "subagents/COM1.txt",
            "subagents/name.",
            "subagents//empty",
        ] {
            assert!(SessionSubpath::new(value).is_err(), "accepted a Windows alias");
        }
    }
    for value in ["..", "../project", "NUL", "project.", "project ", "C:project"] {
        assert!(SessionProjectKey::new(value).is_err(), "accepted unsafe project key");
    }
    for value in ["NUL", "session.", "session ", "../session", "C:session"] {
        assert!(key("project", value).is_err(), "accepted unsafe session key");
    }
    let root = tempfile::tempdir()?;
    assert!(SessionKey::from_transcript_path(&root.path().join("project/../escape.jsonl"), root.path()).is_err());
    let derived = SessionKey::from_transcript_path(
        &root.path().join("project/session/subagents/agent-one.jsonl"),
        root.path(),
    )?;
    assert_eq!(
        derived.subpath().map(SessionSubpath::as_str),
        Some("subagents/agent-one")
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn resume_materializes_main_subagents_and_metadata_without_touching_credentials() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = Arc::new(InMemorySessionStore::new());
    let main = key("project", SESSION)?;
    let sub = main.clone().with_subpath(SessionSubpath::new("subagents/agent-one")?);
    store.append(&main, &[entry("user", 1)?]).await?;
    store
        .append(
            &sub,
            &[
                entry("assistant", 2)?,
                SessionEntry::try_from(json!({"type":"agent_metadata","agentType":"reviewer"}))?,
            ],
        )
        .await?;
    let runtime = SessionRuntime::prepare(
        &options(store, root.path())?,
        StoreResume::Session(SessionId::from(SESSION)),
        root.path(),
        &Environment::default(),
        Default::default(),
    )
    .await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    assert!(!config.starts_with(root.path()));
    let project = config.join("projects/project");
    assert_eq!(runtime.resume_session_id.as_ref().map(SessionId::as_str), Some(SESSION));
    assert!(project.join(format!("{SESSION}.jsonl")).is_file());
    assert!(project.join(SESSION).join("subagents/agent-one.jsonl").is_file());
    let sidecar: serde_json::Value = serde_json::from_slice(&std::fs::read(
        project.join(SESSION).join("subagents/agent-one.meta.json"),
    )?)?;
    assert_eq!(sidecar, json!({"agentType":"reviewer"}));
    assert!(!config.join(".credentials.json").exists());
    drop(runtime);
    assert!(!config.exists());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn w3_resume_leaves_an_existing_caller_transcript_untouched() -> TestResult {
    let root = tempfile::tempdir()?;
    let project = root.path().join("projects/project");
    std::fs::create_dir_all(&project)?;
    let transcript = project.join(format!("{SESSION}.jsonl"));
    std::fs::write(&transcript, b"existing caller transcript")?;
    let store = Arc::new(InMemorySessionStore::new());
    store.append(&key("project", SESSION)?, &[entry("user", 1)?]).await?;
    let runtime = SessionRuntime::prepare(
        &options(store, root.path())?,
        StoreResume::Session(SessionId::from(SESSION)),
        root.path(),
        &Environment::default(),
        Default::default(),
    )
    .await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    assert!(
        config
            .join("projects/project")
            .join(format!("{SESSION}.jsonl"))
            .is_file()
    );
    drop(runtime);
    assert_eq!(std::fs::read(transcript)?, b"existing caller transcript");
    Ok(())
}

#[tokio::test]
async fn configuring_mirror_without_resume_does_not_create_a_config_directory() -> TestResult {
    let root = tempfile::tempdir()?;
    let config_root = root.path().join("not-created");
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = SessionRuntime::prepare(
        &options(store, &config_root)?,
        StoreResume::None,
        root.path(),
        &Environment::default(),
        Default::default(),
    )
    .await?;
    assert_eq!(runtime.config_dir, Some(config_root.clone()));
    assert!(!config_root.exists());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn latest_resume_skips_sidechains_and_materializes_the_newest_main_session() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = Arc::new(InMemorySessionStore::new());
    let main = key("project", SESSION)?;
    let sidechain = key("project", "123e4567-e89b-42d3-a456-426614174001")?;
    store.append(&main, &[entry("user", 1)?]).await?;
    // The store stamps appends with the wall clock in milliseconds; keep the sidechain strictly newer.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    store
        .append(
            &sidechain,
            &[SessionEntry::try_from(json!({"type":"assistant","isSidechain":true}))?],
        )
        .await?;
    // Python walks past sidechains; TypeScript `nnt` resumes the newest listed session as is.
    for (dialect, expected) in [
        (SummaryDialect::Python, SESSION),
        (SummaryDialect::TypeScript, sidechain.session_id().as_str()),
    ] {
        let mut options = options(store.clone(), root.path())?;
        options.dialect = dialect;
        let runtime = SessionRuntime::prepare(
            &options,
            StoreResume::Latest,
            root.path(),
            &Environment::default(),
            Default::default(),
        )
        .await?;
        assert_eq!(
            runtime.resume_session_id.as_ref().map(SessionId::as_str),
            Some(expected)
        );
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn resume_entry_and_byte_limits_refuse_materialization_without_transcript_files() -> TestResult {
    for bytes_limited in [false, true] {
        let root = tempfile::tempdir()?;
        let store = Arc::new(InMemorySessionStore::new());
        store
            .append(&key("project", SESSION)?, &[entry("user", 1)?, entry("assistant", 2)?])
            .await?;
        let mut options = options(store, root.path())?;
        if bytes_limited {
            options.limits.max_resume_bytes = Some(NonZeroUsize::new(2).ok_or("zero bound")?);
        } else {
            options.limits.max_resume_entries = Some(NonZeroUsize::new(1).ok_or("zero bound")?);
        }
        let outcome = SessionRuntime::prepare(
            &options,
            StoreResume::Session(SessionId::from(SESSION)),
            root.path(),
            &Environment::default(),
            Default::default(),
        )
        .await;
        if bytes_limited {
            assert!(matches!(outcome, Err(SessionError::ByteLimit)));
        } else {
            assert!(matches!(outcome, Err(SessionError::EntryLimit)));
        }
        assert!(
            !root
                .path()
                .join("projects/project")
                .join(format!("{SESSION}.jsonl"))
                .exists()
        );
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn resume_subkey_limit_cleans_an_already_seeded_main_transcript() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = Arc::new(InMemorySessionStore::new());
    let main = key("project", SESSION)?;
    store.append(&main, &[entry("user", 1)?]).await?;
    for name in ["subagents/one", "subagents/two"] {
        store
            .append(
                &main.clone().with_subpath(SessionSubpath::new(name)?),
                &[entry("assistant", 2)?],
            )
            .await?;
    }
    let mut options = options(store, root.path())?;
    options.limits.max_subkeys = Some(trogon_claude_agent_sdk::QueueCapacity::new(1)?);
    let outcome = SessionRuntime::prepare(
        &options,
        StoreResume::Session(SessionId::from(SESSION)),
        root.path(),
        &Environment::default(),
        Default::default(),
    )
    .await;
    assert!(matches!(outcome, Err(SessionError::SubpathLimit)));
    assert!(
        !root
            .path()
            .join("projects/project")
            .join(format!("{SESSION}.jsonl"))
            .exists()
    );
    Ok(())
}

struct PendingLoadStore;
impl SessionStore for PendingLoadStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn resume_load_timeout_does_not_install_any_files() -> TestResult {
    let root = tempfile::tempdir()?;
    let mut options = options(Arc::new(PendingLoadStore), root.path())?;
    options.limits.load_timeout = Timeout::new(Duration::from_millis(20));
    let outcome = SessionRuntime::prepare(
        &options,
        StoreResume::Session(SessionId::from(SESSION)),
        root.path(),
        &Environment::default(),
        Default::default(),
    )
    .await;
    assert!(matches!(outcome, Err(SessionError::LoadTimeout(_))));
    assert!(std::fs::read_dir(root.path())?.next().is_none());
    Ok(())
}

#[tokio::test]
async fn mirror_flush_observes_ordered_appends_and_rejects_outside_paths() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = Arc::new(InMemorySessionStore::new());
    let cfg = options(store.clone(), root.path())?;
    let mirror = TranscriptMirror::new(&cfg, root.path().join("projects"));
    let path = root.path().join("projects/project").join(format!("{SESSION}.jsonl"));
    mirror.enqueue(&path, vec![entry("user", 1)?])?;
    mirror.enqueue(&path, vec![entry("assistant", 2)?])?;
    assert!(mirror.flush().await?.is_empty());
    assert_eq!(
        store.load(&key("project", SESSION)?).await?,
        Some(vec![entry("user", 1)?, entry("assistant", 2)?])
    );
    assert!(matches!(
        mirror.enqueue(Path::new("/outside/session.jsonl"), vec![entry("user", 3)?]),
        Err(MirrorEnqueueError::Key(_))
    ));
    Ok(())
}

struct PendingStore {
    calls: Arc<AtomicUsize>,
}
impl SessionStore for PendingStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test(start_paused = true)]
async fn timed_out_mirror_append_is_reported_and_never_retried() -> TestResult {
    let root = memory_mirror_root();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut options = options(Arc::new(PendingStore { calls: calls.clone() }), &root)?;
    options.limits.append_timeout = Timeout::new(Duration::from_millis(20));
    options.limits.flush_timeout = Some(Timeout::new(Duration::from_secs(1)));
    let mirror = TranscriptMirror::new(&options, root.join("projects"));
    mirror.enqueue(
        &root.join("projects/project").join(format!("{SESSION}.jsonl")),
        vec![entry("user", 1)?],
    )?;
    let started = tokio::time::Instant::now();
    let failures = mirror.flush().await?;
    let elapsed = started.elapsed();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(failures.as_slice(), [MirrorFailureError::Timeout { .. }]));
    assert!(
        matches!(failures.into_iter().next().ok_or("missing error")?.into_message()?, trogon_claude_agent_sdk::Message::System(message) if message.subtype == "mirror_error")
    );
    assert!(elapsed >= Duration::from_millis(20));
    assert!(elapsed <= Duration::from_millis(21));
    Ok(())
}

struct FailingStore {
    calls: Arc<AtomicUsize>,
    started: Arc<Mutex<Vec<tokio::time::Instant>>>,
}
impl SessionStore for FailingStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started
                .lock()
                .map_err(|_| SessionStoreError::Poisoned)?
                .push(tokio::time::Instant::now());
            Err(SessionStoreError::backend(std::io::Error::other("fixture failure")))
        })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test(start_paused = true)]
async fn definite_mirror_failure_is_retried_with_a_bounded_attempt_count() -> TestResult {
    let root = memory_mirror_root();
    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Mutex::new(Vec::new()));
    let options = options(
        Arc::new(FailingStore {
            calls: calls.clone(),
            started: started.clone(),
        }),
        &root,
    )?;
    let mirror = TranscriptMirror::new(&options, root.join("projects"));
    mirror.enqueue(
        &root.join("projects/project").join(format!("{SESSION}.jsonl")),
        vec![entry("user", 1)?],
    )?;
    assert!(matches!(
        mirror.flush().await?.as_slice(),
        [MirrorFailureError::Append { .. }]
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let started = started.lock().map_err(|_| SessionStoreError::Poisoned)?;
    assert_eq!(started.len(), 3);
    for (actual, minimum) in [
        (started[1] - started[0], Duration::from_millis(200)),
        (started[2] - started[1], Duration::from_millis(800)),
    ] {
        assert!(actual >= minimum);
        assert!(actual <= minimum + Duration::from_millis(1));
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn delayed_flush_wait_keeps_the_original_deadline() -> TestResult {
    let root = memory_mirror_root();
    let mut options = options(
        Arc::new(PendingStore {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        &root,
    )?;
    options.limits.flush_timeout = Some(Timeout::new(Duration::from_millis(50)));
    let mirror = TranscriptMirror::new(&options, root.join("projects"));
    mirror.enqueue(
        &root.join("projects/project").join(format!("{SESSION}.jsonl")),
        vec![entry("user", 1)?],
    )?;
    let marker = mirror.begin_flush()?;
    tokio::time::advance(Duration::from_millis(100)).await;
    let waited = tokio::time::Instant::now();
    assert!(matches!(marker.wait().await, Err(SessionError::MirrorFlushTimeout)));
    assert_eq!(waited.elapsed(), Duration::ZERO);
    Ok(())
}

#[tokio::test]
async fn synchronous_flush_fences_keep_later_pending_appends_out_of_an_earlier_result() -> TestResult {
    struct GatedStore {
        inner: InMemorySessionStore,
        release: Arc<tokio::sync::Notify>,
    }
    impl SessionStore for GatedStore {
        fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
            Box::pin(async move {
                if entries
                    .first()
                    .and_then(|entry| entry.as_object().get("value"))
                    .and_then(serde_json::Value::as_u64)
                    == Some(2)
                {
                    self.release.notified().await;
                }
                self.inner.append(key, entries).await
            })
        }
        fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
            self.inner.load(key)
        }
    }
    let root = tempfile::tempdir()?;
    let release = Arc::new(tokio::sync::Notify::new());
    let store = Arc::new(GatedStore {
        inner: InMemorySessionStore::new(),
        release: release.clone(),
    });
    let options = options(store.clone(), root.path())?;
    let mirror = TranscriptMirror::new(&options, root.path().join("projects"));
    let path = root.path().join("projects/project").join(format!("{SESSION}.jsonl"));
    mirror.enqueue(&path, vec![entry("user", 1)?])?;
    let first = mirror.begin_flush()?;
    mirror.enqueue(&path, vec![entry("assistant", 2)?])?;
    let second = mirror.begin_flush()?;
    assert!(first.wait().await?.is_empty());
    assert_eq!(
        store.load(&key("project", SESSION)?).await?,
        Some(vec![entry("user", 1)?])
    );
    release.notify_one();
    assert!(second.wait().await?.is_empty());
    assert_eq!(
        store
            .load(&key("project", SESSION)?)
            .await?
            .ok_or("missing transcript")?
            .len(),
        2
    );
    Ok(())
}
