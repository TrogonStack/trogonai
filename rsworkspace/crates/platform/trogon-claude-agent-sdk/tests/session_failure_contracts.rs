#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Session contracts use exact transcript and error-message payloads."
    )
)]

#[path = "support/recording_filesystem.rs"]
mod recording_filesystem;

use std::{
    collections::BTreeMap,
    error::Error as StdError,
    ffi::OsString,
    io,
    num::NonZeroUsize,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use recording_filesystem::RecordingFileSystem;
use serde_json::{Value, json};
use tokio::sync::Notify;
use trogon_claude_agent_sdk::{
    Environment, InMemorySessionStore, Message, MirrorEnqueueError, MirrorFailureError, QueueCapacity, SessionEntry,
    SessionEntryError, SessionError, SessionId, SessionKey, SessionKeyError, SessionPreparationOptions,
    SessionProjectKey, SessionProjectKeyError, SessionRuntime, SessionStore, SessionStoreError, SessionStoreFuture,
    SessionStoreOperation, SessionStoreOptions, SessionSubpath, StoreResume, TranscriptMirror,
    adapters::{CanonicalizePath, SpawnProcess},
    test_support::FixedEnvironment,
};

type TestResult = Result<(), Box<dyn StdError>>;
const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";

fn key() -> Result<SessionKey, Box<dyn StdError>> {
    Ok(SessionKey::new(
        SessionProjectKey::new("project")?,
        SessionId::from(SESSION),
    )?)
}

fn entry() -> Result<SessionEntry, SessionEntryError> {
    SessionEntry::try_from(json!({"type":"assistant","text":"retained"}))
}

fn mirror_path() -> PathBuf {
    transcript_path(Path::new("/memory-config"))
}

fn transcript_path(config: &Path) -> PathBuf {
    config.join("projects/project").join(format!("{SESSION}.jsonl"))
}

#[test]
fn s01_session_entries_accept_any_json_object_and_redact_contents() -> TestResult {
    for value in [Value::Null, json!([]), json!("entry")] {
        assert!(matches!(
            SessionEntry::try_from(value),
            Err(SessionEntryError::NotObject)
        ));
    }
    for value in [
        json!({}),
        json!({"type":""}),
        json!({"type":"user\n"}),
        json!({"type":7}),
        json!({"type":"user","uuid":""}),
        json!({"type":"user","uuid":false}),
    ] {
        let object = value.as_object().ok_or("missing object")?.clone();
        assert_eq!(SessionEntry::try_from(value)?.into_object(), object);
    }
    let value = json!({"type":"assistant","uuid":null,"isSidechain":true,"text":"transcript-content"});
    let object = value.as_object().ok_or("missing object")?.clone();
    let validated = SessionEntry::from(object.clone());
    assert_eq!(validated.as_object(), &object);
    assert!(validated.is_sidechain());
    assert_eq!(validated.uuid(), None);
    assert_eq!(validated.clone().into_object(), object);
    assert_eq!(format!("{validated:?}"), "SessionEntry { kind: \"assistant\", .. }");
    Ok(())
}

/// Both upstream SDKs map any transcript path two or more segments below the session directory to a subpath, with or
/// without a `.jsonl` suffix.
#[test]
fn s25_transcript_paths_below_a_session_map_to_subpaths() -> TestResult {
    let projects = Path::new("/sessions/projects");
    for (path, expected) in [
        (
            "/sessions/projects/project/session/workflows/run-1.jsonl",
            "workflows/run-1",
        ),
        (
            "/sessions/projects/project/session/subagents/agent-1.jsonl",
            "subagents/agent-1",
        ),
        ("/sessions/projects/project/session/tasks/notes", "tasks/notes"),
    ] {
        let key = SessionKey::from_transcript_path(Path::new(path), projects)?;
        assert_eq!(key.subpath().map(ToString::to_string).as_deref(), Some(expected));
    }
    Ok(())
}

#[test]
fn session_keys_preserve_identity_and_reject_unrecognized_or_non_unicode_paths() -> TestResult {
    let main = key()?;
    let subpath = SessionSubpath::new("subagents/nested/agent-one")?;
    let sub = main.clone().with_subpath(subpath.clone());
    assert_eq!(sub.main(), main);
    assert_eq!(sub.subpath(), Some(&subpath));
    assert_eq!(subpath.to_string(), "subagents/nested/agent-one");
    assert_eq!(main.session_id().to_string(), SESSION);
    for unsafe_id in ["", "line\nbreak"] {
        assert!(matches!(
            SessionKey::new(SessionProjectKey::new("project")?, SessionId::from(unsafe_id)),
            Err(SessionKeyError::UnsafeSessionId)
        ));
    }
    let projects = Path::new("/sessions/projects");
    for path in ["/sessions/projects/project", "/sessions/projects/project/session.txt"] {
        assert!(matches!(
            SessionKey::from_transcript_path(Path::new(path), projects),
            Err(SessionKeyError::UnrecognizedPath)
        ));
    }
    assert!(matches!(
        SessionKey::from_transcript_path(Path::new("relative/project/session.jsonl"), projects),
        Err(SessionKeyError::OutsideProjects)
    ));
    assert!(matches!(
        SessionKey::from_transcript_path(
            Path::new("/sessions/projects/project/session.jsonl"),
            Path::new("relative")
        ),
        Err(SessionKeyError::OutsideProjects)
    ));
    let non_unicode = projects.join(OsString::from_vec(vec![0xff]));
    assert!(matches!(
        SessionKey::from_transcript_path(&non_unicode, projects),
        Err(SessionKeyError::NotUnicode)
    ));
    Ok(())
}

struct CanonicalPath(Result<PathBuf, io::ErrorKind>);

impl CanonicalizePath for CanonicalPath {
    type Error = io::Error;
    fn canonicalize(&self, _: &Path) -> io::Result<PathBuf> {
        self.0.clone().map_err(io::Error::from)
    }
}

#[test]
fn project_keys_use_native_paths_and_the_pinned_long_path_hash() -> TestResult {
    let directory = tempfile::tempdir()?;
    let key = SessionProjectKey::for_directory(directory.path())?;
    assert_eq!(key.to_string(), key.as_str());
    let long = format!("/{}", "a".repeat(200));
    let long_key =
        SessionProjectKey::for_directory_with_filesystem(Path::new("ignored"), &CanonicalPath(Ok(long.into())))?;
    assert_eq!(long_key.as_str(), format!("-{}-b6ymvl", "a".repeat(199)));
    assert!(
        matches!(SessionProjectKey::for_directory_with_filesystem(Path::new("ignored"), &CanonicalPath(Err(io::ErrorKind::PermissionDenied))), Err(SessionProjectKeyError::Directory(source)) if source.kind() == io::ErrorKind::PermissionDenied)
    );
    assert!(matches!(
        SessionProjectKey::for_directory_with_filesystem(
            Path::new("ignored"),
            &CanonicalPath(Ok(PathBuf::from(OsString::from_vec(vec![0xff]))))
        ),
        Err(SessionProjectKeyError::NotUnicode)
    ));
    Ok(())
}

struct BasicStore;

impl SessionStore for BasicStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test]
async fn optional_store_operations_are_explicit_and_debug_output_hides_configuration() -> TestResult {
    let store = BasicStore;
    let main = key()?;
    assert!(matches!(
        store.list_sessions(main.project_key()).await,
        Err(SessionStoreError::Unsupported(SessionStoreOperation::List))
    ));
    assert!(matches!(
        store.delete(&main).await,
        Err(SessionStoreError::Unsupported(SessionStoreOperation::Delete))
    ));
    assert!(store.list_subkeys(&main).await?.is_empty());
    for (operation, expected) in [
        (SessionStoreOperation::Append, "append"),
        (SessionStoreOperation::Load, "load"),
        (SessionStoreOperation::List, "list"),
        (SessionStoreOperation::Delete, "delete"),
        (SessionStoreOperation::ListSubkeys, "list subkeys"),
    ] {
        assert_eq!(operation.to_string(), expected);
    }
    let mut options = SessionStoreOptions::from(Arc::new(store));
    options.config_directory = Some("/configuration-not-for-debug".into());
    assert!(format!("{options:?}").starts_with("SessionStoreOptions { limits:"));
    assert!(!format!("{options:?}").contains("configuration-not-for-debug"));
    let memory = InMemorySessionStore::new();
    assert_eq!(format!("{memory:?}"), "InMemorySessionStore { .. }");
    for error in [
        SessionStoreError::backend(io::Error::from(io::ErrorKind::PermissionDenied)),
        SessionStoreError::backend(io::Error::from(io::ErrorKind::WouldBlock)),
    ] {
        assert_eq!(format!("{error:?}"), "SessionStoreError { .. }");
        assert!(error.source().is_some());
    }
    Ok(())
}

#[test]
fn mirror_failure_messages_preserve_context_and_bound_suppressed_failures() -> TestResult {
    let main = key()?;
    let child = main.clone().with_subpath(SessionSubpath::new("subagents/agent-one")?);
    for (failure, expected_key) in [
        (
            MirrorFailureError::Append {
                key: main.clone(),
                source: SessionStoreError::backend(io::Error::from(io::ErrorKind::PermissionDenied)),
            },
            Some(main.clone()),
        ),
        (
            MirrorFailureError::Append {
                key: child.clone(),
                source: SessionStoreError::backend(io::Error::from(io::ErrorKind::PermissionDenied)),
            },
            Some(child),
        ),
        (MirrorFailureError::Suppressed { count: 3 }, None),
    ] {
        assert_eq!(failure.key(), expected_key.as_ref());
        let Message::System(message) = failure.into_message()? else {
            return Err("unexpected mirror message".into());
        };
        assert_eq!(message.subtype, "mirror_error");
        if let Some(key) = expected_key {
            assert_eq!(message.data["key"].get("subpath").is_some(), key.subpath().is_some());
            assert_eq!(message.data["key"]["project_key"], key.project_key().as_str());
            assert_eq!(message.data["key"]["session_id"], key.session_id().as_str());
            if let Some(path) = key.subpath() {
                assert_eq!(message.data["key"]["subpath"], path.as_str());
            }
            assert!(message.data["error"].as_str().is_some_and(|text| !text.is_empty()));
            assert_eq!(message.data["uuid"].as_str().map(str::len), Some(36));
            assert_eq!(message.session_id, Some(key.session_id().clone()));
        } else {
            assert_eq!(message.data["suppressed_count"], 3);
            assert!(message.data.get("key").is_none());
            assert!(message.session_id.is_none());
        }
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn s19_mirror_never_rejects_frames_and_drains_eagerly_past_the_pending_budget() -> TestResult {
    let store = Arc::new(InMemorySessionStore::new());
    let mut options = SessionStoreOptions::from(store.clone());
    options.limits.max_retained_failures = Some(QueueCapacity::new(1)?);
    options.limits.max_pending_entries = NonZeroUsize::new(2).ok_or("zero limit")?;
    options.limits.max_pending_bytes = NonZeroUsize::new(1).ok_or("zero limit")?;
    let mirror = TranscriptMirror::new(&options, PathBuf::from("/memory-config/projects"));
    assert_eq!(format!("{mirror:?}"), "TranscriptMirror { .. }");
    mirror.enqueue(&mirror_path(), Vec::new())?;
    for _ in 0..4 {
        mirror.enqueue(&mirror_path(), vec![entry()?, entry()?, entry()?])?;
    }
    let mut loaded = 0;
    while loaded < 12 {
        // Under paused time a sleep lets the clock advance while the mirror task runs; yielding
        // would keep this task runnable and starve the mirror's own timers.
        tokio::time::sleep(Duration::from_millis(1)).await;
        loaded = store.load(&key()?).await?.map_or(0, |entries| entries.len());
    }
    assert!(mirror.flush().await?.is_empty());
    assert_eq!(store.load(&key()?).await?.ok_or("missing entries")?.len(), 12);
    Ok(())
}

struct FailedAppendStore {
    calls: AtomicUsize,
    observed: Notify,
}

impl SessionStore for FailedAppendStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.observed.notify_one();
            Err(SessionStoreError::backend(io::Error::from(
                io::ErrorKind::PermissionDenied,
            )))
        })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test(start_paused = true)]
async fn w1_mirror_retries_every_backend_error_and_coalesces_failure_overflow() -> TestResult {
    let store = Arc::new(FailedAppendStore {
        calls: AtomicUsize::new(0),
        observed: Notify::new(),
    });
    let mut options = SessionStoreOptions::from(store.clone());
    options.limits.max_retained_failures = Some(QueueCapacity::new(1)?);
    let mirror = TranscriptMirror::new(&options, PathBuf::from("/memory-config/projects"));
    let session = mirror_path().with_extension("");
    mirror.enqueue(&mirror_path(), vec![entry()?])?;
    mirror.enqueue(&session.join("subagents/agent-one.jsonl"), vec![entry()?])?;
    mirror.enqueue(&session.join("subagents/agent-two.jsonl"), vec![entry()?])?;
    mirror.enqueue(&mirror_path(), vec![entry()?])?;
    let failures = mirror.flush().await?;
    assert!(matches!(
        failures.as_slice(),
        [
            MirrorFailureError::Append { .. },
            MirrorFailureError::Suppressed { count: 2 }
        ]
    ));
    assert_eq!(store.calls.load(Ordering::SeqCst), 9);
    assert!(mirror.flush().await?.is_empty());
    Ok(())
}

struct PanickingAppendStore;

impl SessionStore for PanickingAppendStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async { std::panic::resume_unwind(Box::new("fixture backend panic")) })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test(start_paused = true)]
async fn mirror_reports_a_stopped_worker_through_each_public_wait() -> TestResult {
    let options = SessionStoreOptions::from(Arc::new(PanickingAppendStore));
    let mirror = TranscriptMirror::new(&options, PathBuf::from("/memory-config/projects"));
    mirror.enqueue(&mirror_path(), vec![entry()?])?;
    let marker = mirror.begin_flush()?;
    assert!(matches!(marker.wait().await, Err(SessionError::MirrorStopped)));
    assert!(matches!(
        mirror.enqueue(&mirror_path(), vec![entry()?]),
        Err(MirrorEnqueueError::Closed)
    ));
    assert!(matches!(mirror.begin_flush(), Err(SessionError::MirrorStopped)));
    assert!(matches!(mirror.flush().await, Err(SessionError::MirrorStopped)));
    Ok(())
}

struct FixedClock(SystemTime);

impl trogon_claude_agent_sdk::adapters::WallClock for FixedClock {
    type Time = SystemTime;
    fn now(&self) -> SystemTime {
        self.0
    }
}

struct PanickingClock;

impl trogon_claude_agent_sdk::adapters::WallClock for PanickingClock {
    type Time = SystemTime;
    fn now(&self) -> SystemTime {
        std::panic::resume_unwind(Box::new("fixture clock panic"))
    }
}

#[tokio::test]
async fn memory_store_reports_clock_exhaustion_and_poisoning_as_non_retryable() -> TestResult {
    let maximum = UNIX_EPOCH
        .checked_add(Duration::from_millis(u64::MAX))
        .ok_or("clock cannot represent fixture")?;
    let store = InMemorySessionStore::with_clock(FixedClock(maximum));
    let main = key()?;
    store.append(&main, &[entry()?]).await?;
    let exhausted = store.append(&main, &[entry()?]).await;
    assert!(matches!(exhausted, Err(SessionStoreError::ClockExhausted)));
    let store = Arc::new(InMemorySessionStore::with_clock(PanickingClock));
    let worker_store = store.clone();
    let worker_key = main.clone();
    let worker = tokio::spawn(async move {
        let value = entry().map_err(Box::<dyn StdError + Send + Sync>::from)?;
        worker_store
            .append(&worker_key, &[value])
            .await
            .map_err(Box::<dyn StdError + Send + Sync>::from)
    });
    assert!(worker.await.is_err_and(|error| error.is_panic()));
    assert!(matches!(store.load(&main).await, Err(SessionStoreError::Poisoned)));
    assert!(matches!(
        store.append(&main, &[entry()?]).await,
        Err(SessionStoreError::Poisoned)
    ));
    assert!(matches!(
        store.list_sessions(main.project_key()).await,
        Err(SessionStoreError::Poisoned)
    ));
    assert!(matches!(store.delete(&main).await, Err(SessionStoreError::Poisoned)));
    assert!(matches!(
        store.list_subkeys(&main).await,
        Err(SessionStoreError::Poisoned)
    ));
    Ok(())
}

fn resume_options(store: Arc<dyn SessionStore>, config: &Path) -> Result<SessionStoreOptions, Box<dyn StdError>> {
    let mut options = SessionStoreOptions::from(store);
    options.config_directory = Some(config.to_path_buf());
    options.project_key = Some(SessionProjectKey::new("project")?);
    Ok(options)
}

/// Refuses every spawn, so no test reads a real keychain.
struct NoProcess;

impl SpawnProcess for NoProcess {
    type Error = io::Error;
    fn spawn(&self, _: &mut tokio::process::Command) -> io::Result<tokio::process::Child> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

async fn prepare(
    options: &SessionStoreOptions,
    resume: StoreResume,
    filesystem: RecordingFileSystem,
) -> Result<SessionRuntime<RecordingFileSystem>, SessionError> {
    let environment = FixedEnvironment {
        directory: "/workspace".into(),
        variables: BTreeMap::from([("TMPDIR".into(), filesystem.path("").into_os_string())]),
    };
    SessionRuntime::prepare_with_adapters(SessionPreparationOptions {
        store_options: options,
        resume,
        cwd: &environment.directory,
        subprocess_environment: &Environment::default(),
        environment: &environment,
        process: &NoProcess,
        filesystem,
    })
    .await
}

/// Lists the `claude-resume-*` directories the runtime left in the temporary root.
fn resume_directories(filesystem: &RecordingFileSystem) -> io::Result<Vec<PathBuf>> {
    std::fs::read_dir(filesystem.path(""))?
        .map(|entry| entry.map(|entry| entry.path()))
        .filter(|path| {
            path.as_ref().map_or(true, |path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("claude-resume-"))
            })
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn resume_rejects_relative_configuration_and_requires_listing_for_latest() -> TestResult {
    let store = Arc::new(BasicStore);
    let filesystem = RecordingFileSystem::new()?;
    let config = filesystem.path("config");
    let mut options = resume_options(store, &config)?;
    for path in ["", "relative"] {
        options.config_directory = Some(path.into());
        assert!(matches!(
            prepare(&options, StoreResume::None, filesystem.clone()).await,
            Err(SessionError::UnsafeConfigPath)
        ));
    }
    options.config_directory = Some(config);
    assert!(matches!(
        prepare(&options, StoreResume::Latest, filesystem.clone()).await,
        Err(SessionError::Store(SessionStoreError::Unsupported(
            SessionStoreOperation::List
        )))
    ));
    assert!(resume_directories(&filesystem)?.is_empty());
    Ok(())
}

struct DeclaredSubkeyStore {
    inner: InMemorySessionStore,
    subpath: String,
}

impl SessionStore for DeclaredSubkeyStore {
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        self.inner.append(key, entries)
    }
    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        self.inner.load(key)
    }
    fn list_subkeys<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Vec<String>> {
        Box::pin(async { Ok(vec![self.subpath.clone()]) })
    }
}

#[tokio::test(start_paused = true)]
async fn resume_skips_declared_subkeys_without_a_transcript() -> TestResult {
    let store = Arc::new(DeclaredSubkeyStore {
        inner: InMemorySessionStore::new(),
        subpath: "subagents/not-present".into(),
    });
    store.append(&key()?, &[entry()?]).await?;
    let filesystem = RecordingFileSystem::new()?;
    let config = filesystem.path("config");
    let options = resume_options(store, &config)?;
    let runtime = prepare(
        &options,
        StoreResume::Session(SessionId::from(SESSION)),
        filesystem.clone(),
    )
    .await?;
    assert_eq!(runtime.resume_session_id, Some(SessionId::from(SESSION)));
    let materialized = runtime.config_dir.clone().ok_or("missing config directory")?;
    assert!(filesystem.read(&transcript_path(&materialized)).is_ok());
    assert!(!filesystem.exists(&transcript_path(&config)));
    assert!(
        !filesystem.exists(
            &materialized
                .join("projects/project")
                .join(SESSION)
                .join("subagents/not-present.jsonl")
        )
    );
    assert_eq!(
        format!("{:?}", runtime.materialized.as_ref().ok_or("missing lease")?),
        format!("MaterializedSession {{ path: {materialized:?} }}")
    );
    drop(runtime);
    assert!(!filesystem.exists(&materialized));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn latest_resume_limits_the_candidate_listing_before_materialization() -> TestResult {
    let store = Arc::new(InMemorySessionStore::new());
    store.append(&key()?, &[entry()?]).await?;
    let other = SessionKey::new(
        SessionProjectKey::new("project")?,
        SessionId::from(uuid::Uuid::nil().to_string()),
    )?;
    store.append(&other, &[entry()?]).await?;
    let filesystem = RecordingFileSystem::new()?;
    let config = filesystem.path("config");
    let mut options = resume_options(store, &config)?;
    options.limits.max_resume_entries = NonZeroUsize::new(1).ok_or("zero limit")?;
    assert!(matches!(
        prepare(&options, StoreResume::Latest, filesystem.clone()).await,
        Err(SessionError::EntryLimit)
    ));
    assert!(resume_directories(&filesystem)?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn resume_counts_transcript_newlines_and_metadata_against_budgets() -> TestResult {
    let main = key()?;
    let transcript = entry()?;
    let encoded_length = serde_json::to_vec(transcript.as_object())?.len();
    let store = Arc::new(InMemorySessionStore::new());
    store.append(&main, std::slice::from_ref(&transcript)).await?;
    let filesystem = RecordingFileSystem::new()?;
    let mut options = resume_options(store.clone(), &filesystem.path("config"))?;
    options.limits.max_resume_bytes = NonZeroUsize::new(encoded_length).ok_or("zero limit")?;
    assert!(matches!(
        prepare(&options, StoreResume::Session(SessionId::from(SESSION)), filesystem).await,
        Err(SessionError::ByteLimit)
    ));
    let metadata = SessionEntry::try_from(json!({"type":"agent_metadata","agentType":"reviewer"}))?;
    store
        .append(
            &main.with_subpath(SessionSubpath::new("subagents/metadata-only")?),
            &[metadata],
        )
        .await?;
    for entry_limit in [true, false] {
        let filesystem = RecordingFileSystem::new()?;
        let config = filesystem.path("config");
        let mut options = resume_options(store.clone(), &config)?;
        if entry_limit {
            options.limits.max_resume_entries = NonZeroUsize::new(1).ok_or("zero limit")?;
        } else {
            options.limits.max_resume_bytes = NonZeroUsize::new(encoded_length + 2).ok_or("zero limit")?;
        }
        let outcome = prepare(
            &options,
            StoreResume::Session(SessionId::from(SESSION)),
            filesystem.clone(),
        )
        .await;
        if entry_limit {
            assert!(matches!(outcome, Err(SessionError::EntryLimit)));
        } else {
            assert!(matches!(outcome, Err(SessionError::ByteLimit)));
        }
        assert!(resume_directories(&filesystem)?.is_empty());
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_outstanding_flush_survives_the_final_mirror_owner() -> TestResult {
    let store = Arc::new(InMemorySessionStore::new());
    let options = SessionStoreOptions::from(store.clone());
    let mirror = TranscriptMirror::new(&options, PathBuf::from("/memory-config/projects"));
    mirror.enqueue(&mirror_path(), vec![entry()?])?;
    let marker = mirror.begin_flush()?;
    drop(mirror);
    assert!(marker.wait().await?.is_empty());
    assert_eq!(store.load(&key()?).await?, Some(vec![entry()?]));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn resume_preserves_single_component_subpaths_and_their_metadata() -> TestResult {
    let store = Arc::new(InMemorySessionStore::new());
    let main = key()?;
    store.append(&main, &[entry()?]).await?;
    let child = SessionEntry::try_from(json!({"type":"assistant","text":"child"}))?;
    store
        .append(
            &main.clone().with_subpath(SessionSubpath::new("standalone")?),
            &[
                child.clone(),
                SessionEntry::try_from(json!({"type":"agent_metadata","agentType":"worker"}))?,
            ],
        )
        .await?;
    store
        .append(
            &main.clone().with_subpath(SessionSubpath::new("transcript-only")?),
            std::slice::from_ref(&child),
        )
        .await?;
    let filesystem = RecordingFileSystem::new()?;
    let config = filesystem.path("config");
    let options = resume_options(store.clone(), &config)?;
    let runtime = prepare(
        &options,
        StoreResume::Session(SessionId::from(SESSION)),
        filesystem.clone(),
    )
    .await?;
    let directory = runtime
        .config_dir
        .clone()
        .ok_or("missing config directory")?
        .join("projects/project")
        .join(SESSION);
    let transcript_path = directory.join("standalone.jsonl");
    let metadata_path = directory.join("standalone.meta.json");
    assert_eq!(
        serde_json::from_slice::<Value>(&filesystem.read(&transcript_path)?)?,
        Value::Object(child.into_object())
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&filesystem.read(&metadata_path)?)?,
        json!({"agentType":"worker"})
    );
    let transcript_only = directory.join("transcript-only.jsonl");
    assert_eq!(
        serde_json::from_slice::<Value>(&filesystem.read(&transcript_only)?)?,
        json!({"type":"assistant","text":"child"})
    );
    assert!(!filesystem.exists(&directory.join("transcript-only.meta.json")));
    drop(runtime);
    assert!(!filesystem.exists(&transcript_path));
    assert!(!filesystem.exists(&metadata_path));
    assert!(!filesystem.exists(&transcript_only));
    assert_eq!(store.load(&main).await?, Some(vec![entry()?]));
    Ok(())
}
