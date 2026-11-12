#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Filesystem adapter tests construct transcript protocol entries and configuration files."
    )
)]

#[path = "support/recording_filesystem.rs"]
mod recording_filesystem;

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use recording_filesystem::{Operation, RecordingFileSystem};
use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    Environment, InMemorySessionStore, SessionEntry, SessionError, SessionId, SessionKey, SessionListEntry,
    SessionPreparationOptions, SessionProjectKey, SessionRuntime, SessionStore, SessionStoreFuture,
    SessionStoreOptions, SessionSubpath, StoreResume,
    adapters::{SpawnProcess, WallClock},
    test_support::FixedEnvironment,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";

struct FrozenClock;
impl WallClock for FrozenClock {
    type Time = SystemTime;
    fn now(&self) -> SystemTime {
        UNIX_EPOCH
    }
}

/// Records each spawn and refuses it, so no test ever reads a real keychain.
#[derive(Clone, Default)]
struct RefusingProcess {
    commands: Arc<Mutex<Vec<Vec<OsString>>>>,
}

impl RefusingProcess {
    fn commands(&self) -> io::Result<Vec<Vec<OsString>>> {
        Ok(self
            .commands
            .lock()
            .map_err(|_| io::Error::other("process recording poisoned"))?
            .clone())
    }
}

impl SpawnProcess for RefusingProcess {
    type Error = io::Error;
    fn spawn(&self, command: &mut tokio::process::Command) -> io::Result<tokio::process::Child> {
        let std = command.as_std();
        let mut line = vec![std.get_program().to_owned()];
        line.extend(std.get_args().map(OsStr::to_owned));
        self.commands
            .lock()
            .map_err(|_| io::Error::other("process recording poisoned"))?
            .push(line);
        Err(io::Error::from(io::ErrorKind::NotFound))
    }
}

fn entry(value: Value) -> Result<SessionEntry, Box<dyn std::error::Error>> {
    Ok(SessionEntry::try_from(value)?)
}

fn mode(path: &Path) -> io::Result<u32> {
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
}

struct Fixture {
    filesystem: RecordingFileSystem,
    environment: FixedEnvironment,
    options: SessionStoreOptions,
    store: Arc<InMemorySessionStore<FrozenClock>>,
    project: SessionProjectKey,
    process: RefusingProcess,
    subprocess_environment: Environment,
}

impl Fixture {
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let filesystem = RecordingFileSystem::new()?;
        filesystem.create_dir_all(&filesystem.path("project"))?;
        filesystem.create_dir_all(&filesystem.path("tmp"))?;
        let environment = FixedEnvironment {
            directory: filesystem.path("project"),
            variables: BTreeMap::from([
                (
                    OsString::from("CLAUDE_CONFIG_DIR"),
                    filesystem.path("config").into_os_string(),
                ),
                (OsString::from("TMPDIR"), filesystem.path("tmp").into_os_string()),
            ]),
        };
        let project = SessionProjectKey::for_directory_with_filesystem(&environment.directory, &filesystem)?;
        let key = SessionKey::new(project.clone(), SessionId::from(SESSION))?;
        let store = Arc::new(InMemorySessionStore::with_clock(FrozenClock));
        store
            .append(&key, &[entry(json!({"type":"user","message":"hello"}))?])
            .await?;
        store
            .append(
                &key.with_subpath(SessionSubpath::new("subagents/agent-one")?),
                &[
                    entry(json!({"type":"assistant","message":"child"}))?,
                    entry(json!({"type":"agent_metadata","agentType":"reviewer"}))?,
                ],
            )
            .await?;
        Ok(Self {
            filesystem,
            environment,
            options: SessionStoreOptions::from(store.clone() as Arc<dyn SessionStore>),
            store,
            project,
            process: RefusingProcess::default(),
            subprocess_environment: Environment::default(),
        })
    }

    async fn prepare(&self, resume: StoreResume) -> Result<SessionRuntime<RecordingFileSystem>, SessionError> {
        SessionRuntime::prepare_with_adapters(SessionPreparationOptions {
            store_options: &self.options,
            resume,
            cwd: &self.environment.directory,
            subprocess_environment: &self.subprocess_environment,
            environment: &self.environment,
            process: &self.process,
            filesystem: self.filesystem.clone(),
        })
        .await
    }

    async fn resume(&self) -> Result<SessionRuntime<RecordingFileSystem>, SessionError> {
        self.prepare(StoreResume::Session(SessionId::from(SESSION))).await
    }

    fn temporary_entries(&self) -> io::Result<Vec<PathBuf>> {
        std::fs::read_dir(self.filesystem.path("tmp"))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }
}

#[tokio::test]
async fn w3_resume_materializes_into_a_private_temporary_directory_and_cleanup_removes_it() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture.resume().await?;
    let lease = runtime.materialized.as_ref().ok_or("missing materialized session")?;
    let config = lease.config_dir().to_path_buf();
    assert_eq!(config.parent(), Some(fixture.filesystem.path("tmp").as_path()));
    assert!(
        config
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with("claude-resume-"))
    );
    assert_eq!(runtime.config_dir.as_deref(), Some(config.as_path()));
    assert_eq!(runtime.projects_dir, config.join("projects"));
    assert_eq!(runtime.resume_session_id, Some(SessionId::from(SESSION)));
    let project_dir = config.join("projects").join(fixture.project.as_str());
    let transcript = project_dir.join(format!("{SESSION}.jsonl"));
    let child = project_dir.join(SESSION).join("subagents/agent-one.jsonl");
    let metadata = project_dir.join(SESSION).join("subagents/agent-one.meta.json");
    let transcript_bytes = fixture.filesystem.read(&transcript)?;
    assert_eq!(transcript_bytes.last(), Some(&b'\n'));
    assert_eq!(
        serde_json::from_slice::<Value>(&transcript_bytes)?,
        json!({"type":"user","message":"hello"})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fixture.filesystem.read(&child)?)?["message"],
        "child"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fixture.filesystem.read(&metadata)?)?,
        json!({"agentType":"reviewer"})
    );
    for directory in [&config, &project_dir] {
        assert_eq!(mode(directory)?, 0o700);
    }
    for file in [&transcript, &child, &metadata] {
        assert_eq!(mode(file)?, 0o600);
    }
    assert!(!fixture.filesystem.exists(&fixture.filesystem.path("config/projects")));
    let SessionRuntime { materialized, .. } = runtime;
    materialized.ok_or("missing materialized session")?.cleanup().await;
    assert!(!fixture.filesystem.exists(&config));
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?, [config]);
    Ok(())
}

#[tokio::test]
async fn w3_dropping_the_runtime_removes_the_temporary_directory_once() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    drop(runtime);
    assert!(!fixture.filesystem.exists(&config));
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?, [config]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn w3_cleanup_retries_a_busy_directory_and_then_removes_it() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    fixture
        .filesystem
        .fail(Operation::RemoveDirectoryAll, None, io::ErrorKind::ResourceBusy, 2)?;
    let SessionRuntime { materialized, .. } = runtime;
    materialized.ok_or("missing materialized session")?.cleanup().await;
    assert!(!fixture.filesystem.exists(&config));
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?.len(), 3);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn w3_cancelling_cleanup_mid_retry_still_removes_the_directory_on_drop() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    fixture
        .filesystem
        .fail(Operation::RemoveDirectoryAll, None, io::ErrorKind::ResourceBusy, 1)?;
    let SessionRuntime { materialized, .. } = runtime;
    let cleanup = materialized.ok_or("missing materialized session")?.cleanup();
    let outcome = tokio::time::timeout(std::time::Duration::from_millis(1), cleanup).await;
    assert!(outcome.is_err(), "cleanup must still be sleeping between attempts");
    assert!(!fixture.filesystem.exists(&config));
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?.len(), 2);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn w3_cleanup_gives_up_after_bounded_attempts_without_failing() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    fixture.filesystem.fail(
        Operation::RemoveDirectoryAll,
        None,
        io::ErrorKind::ResourceBusy,
        usize::MAX,
    )?;
    let SessionRuntime { materialized, .. } = runtime;
    materialized.ok_or("missing materialized session")?.cleanup().await;
    assert!(fixture.filesystem.exists(&config));
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?.len(), 5);
    std::fs::remove_dir_all(&config)?;
    Ok(())
}

#[tokio::test]
async fn w3_temporary_root_creation_failure_is_reported_without_a_lease() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture
        .filesystem
        .fail(Operation::CreateDirectory, None, io::ErrorKind::PermissionDenied, 1)?;
    assert!(
        matches!(fixture.resume().await, Err(SessionError::Io(source)) if source.kind() == io::ErrorKind::PermissionDenied)
    );
    assert!(fixture.temporary_entries()?.is_empty());
    assert!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn w3_transcript_write_failure_removes_the_partial_temporary_directory() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.filesystem.fail(
        Operation::Open,
        Some(&format!("{SESSION}.jsonl")),
        io::ErrorKind::PermissionDenied,
        1,
    )?;
    assert!(
        matches!(fixture.resume().await, Err(SessionError::Io(source)) if source.kind() == io::ErrorKind::PermissionDenied)
    );
    assert!(fixture.temporary_entries()?.is_empty());
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn w3_resume_copies_auth_and_settings_without_refresh_tokens_or_plugins() -> TestResult {
    let fixture = Fixture::new().await?;
    let source = fixture.filesystem.path("config");
    fixture.filesystem.write_file(
        &source.join(".credentials.json"),
        &serde_json::to_vec(
            &json!({"claudeAiOauth":{"accessToken":"fixture-access","refreshToken":"fixture-refresh","expiresAt":1}}),
        )?,
    )?;
    fixture.filesystem.write_file(
        &source.join("settings.json"),
        &serde_json::to_vec(&json!({
            "model":"fixture",
            "enabledPlugins":{"plugin":true},
            "extraKnownMarketplaces":{"market":{}},
            "env":{"CLAUDE_CONFIG_DIR":"/elsewhere","KEEP":"1"}
        }))?,
    )?;
    fixture
        .filesystem
        .write_file(&source.join("cowork_settings.json"), b"{\"untouched\":true}")?;
    fixture
        .filesystem
        .write_file(&source.join(".claude.json"), b"{\"userID\":\"fixture\"}")?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    let credentials: Value = serde_json::from_slice(&fixture.filesystem.read(&config.join(".credentials.json"))?)?;
    assert_eq!(
        credentials,
        json!({"claudeAiOauth":{"accessToken":"fixture-access","expiresAt":1}})
    );
    let settings: Value = serde_json::from_slice(&fixture.filesystem.read(&config.join("settings.json"))?)?;
    assert_eq!(settings, json!({"model":"fixture","env":{"KEEP":"1"}}));
    assert_eq!(
        fixture.filesystem.read(&config.join("cowork_settings.json"))?,
        b"{\"untouched\":true}"
    );
    assert_eq!(
        fixture.filesystem.read(&config.join(".claude.json"))?,
        b"{\"userID\":\"fixture\"}"
    );
    for name in [
        ".credentials.json",
        "settings.json",
        "cowork_settings.json",
        ".claude.json",
    ] {
        assert_eq!(mode(&config.join(name))?, 0o600);
    }
    assert!(fixture.process.commands()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn w3_an_unreadable_optional_configuration_file_is_skipped() -> TestResult {
    let fixture = Fixture::new().await?;
    let source = fixture.filesystem.path("config");
    fixture
        .filesystem
        .write_file(&source.join(".credentials.json"), b"{}")?;
    fixture
        .filesystem
        .write_file(&source.join("settings.json"), b"{\"model\":\"fixture\"}")?;
    fixture.filesystem.fail(
        Operation::Metadata,
        Some("config/.credentials.json"),
        io::ErrorKind::PermissionDenied,
        1,
    )?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    assert!(!fixture.filesystem.exists(&config.join(".credentials.json")));
    assert!(fixture.filesystem.exists(&config.join("settings.json")));
    Ok(())
}

#[tokio::test]
async fn w3_a_failed_optional_configuration_copy_is_removed_and_resume_continues() -> TestResult {
    let fixture = Fixture::new().await?;
    let source = fixture.filesystem.path("config");
    fixture
        .filesystem
        .write_file(&source.join("settings.json"), b"{\"model\":\"fixture\"}")?;
    fixture.filesystem.fail_after(
        Operation::Open,
        Some("settings.json"),
        io::ErrorKind::PermissionDenied,
        1,
        1,
    )?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    let destination = config.join("settings.json");
    assert!(!fixture.filesystem.exists(&destination));
    assert_eq!(fixture.filesystem.paths(Operation::RemoveFile)?, [destination]);
    assert_eq!(runtime.resume_session_id, Some(SessionId::from(SESSION)));
    Ok(())
}

#[tokio::test]
async fn w3_keychain_credentials_are_requested_only_without_configured_auth() -> TestResult {
    let mut fixture = Fixture::new().await?;
    fixture.environment.variables.remove(OsStr::new("CLAUDE_CONFIG_DIR"));
    fixture
        .environment
        .variables
        .insert(OsString::from("HOME"), fixture.filesystem.path("home").into_os_string());
    fixture
        .environment
        .variables
        .insert(OsString::from("USER"), OsString::from("fixture-user"));
    drop(fixture.resume().await?);
    let expected: Vec<Vec<OsString>> = if cfg!(target_os = "macos") {
        vec![
            [
                "security",
                "find-generic-password",
                "-a",
                "fixture-user",
                "-w",
                "-s",
                "Claude Code-credentials",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
        ]
    } else {
        Vec::new()
    };
    assert_eq!(fixture.process.commands()?, expected);
    fixture.subprocess_environment = Environment::from([("ANTHROPIC_API_KEY".into(), "fixture-key".into())]);
    drop(fixture.resume().await?);
    assert_eq!(fixture.process.commands()?, expected);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn w3_cleanup_treats_an_already_removed_directory_as_done() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    fixture
        .filesystem
        .fail(Operation::RemoveDirectoryAll, None, io::ErrorKind::NotFound, 1)?;
    let SessionRuntime { materialized, .. } = runtime;
    materialized.ok_or("missing materialized session")?.cleanup().await;
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?.len(), 1);
    std::fs::remove_dir_all(&config)?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn w3_cleanup_stops_retrying_after_a_permanent_failure_and_tries_once_more() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    fixture
        .filesystem
        .fail(Operation::RemoveDirectoryAll, None, io::ErrorKind::Other, 1)?;
    let SessionRuntime { materialized, .. } = runtime;
    materialized.ok_or("missing materialized session")?.cleanup().await;
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?.len(), 2);
    assert!(!fixture.filesystem.exists(&config));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn w3_cleanup_reports_a_permanent_failure_without_failing() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    fixture
        .filesystem
        .fail(Operation::RemoveDirectoryAll, None, io::ErrorKind::Other, usize::MAX)?;
    let SessionRuntime { materialized, .. } = runtime;
    materialized.ok_or("missing materialized session")?.cleanup().await;
    assert_eq!(fixture.filesystem.paths(Operation::RemoveDirectoryAll)?.len(), 2);
    assert!(fixture.filesystem.exists(&config));
    std::fs::remove_dir_all(&config)?;
    Ok(())
}

#[tokio::test]
async fn w3_a_relative_home_directory_is_rejected() -> TestResult {
    let mut fixture = Fixture::new().await?;
    fixture.environment.variables.remove(OsStr::new("CLAUDE_CONFIG_DIR"));
    fixture
        .environment
        .variables
        .insert(OsString::from("HOME"), OsString::from("relative/home"));
    assert!(matches!(fixture.resume().await, Err(SessionError::UnsafeConfigPath)));
    Ok(())
}

#[tokio::test]
async fn w3_a_credentials_write_failure_is_reported_and_removes_the_temporary_directory() -> TestResult {
    let fixture = Fixture::new().await?;
    let source = fixture.filesystem.path("config");
    fixture
        .filesystem
        .write_file(&source.join(".credentials.json"), b"{}")?;
    fixture.filesystem.fail_after(
        Operation::Open,
        Some(".credentials.json"),
        io::ErrorKind::PermissionDenied,
        1,
        1,
    )?;
    assert!(matches!(fixture.resume().await, Err(SessionError::Io(_))));
    assert!(fixture.temporary_entries()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn w3_credentials_that_are_not_a_regular_file_are_skipped() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture
        .filesystem
        .create_dir_all(&fixture.filesystem.path("config/.credentials.json"))?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    assert!(!fixture.filesystem.exists(&config.join(".credentials.json")));
    Ok(())
}

#[tokio::test]
async fn w3_configuration_that_needs_no_rewriting_is_copied_verbatim() -> TestResult {
    let fixture = Fixture::new().await?;
    let source = fixture.filesystem.path("config");
    let files: [(&str, &[u8]); 3] = [
        (".credentials.json", b"not json"),
        ("settings.json", b"[1]"),
        ("cowork_settings.json", b"\xEF\xBB\xBF{\"model\":\"fixture\"}"),
    ];
    for (name, contents) in files {
        fixture.filesystem.write_file(&source.join(name), contents)?;
    }
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    for (name, contents) in files {
        assert_eq!(fixture.filesystem.read(&config.join(name))?, contents);
    }
    fixture.filesystem.write_file(
        &source.join(".credentials.json"),
        b"{\"claudeAiOauth\":{\"accessToken\":\"fixture-access\"}}",
    )?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    assert_eq!(
        fixture.filesystem.read(&config.join(".credentials.json"))?,
        b"{\"claudeAiOauth\":{\"accessToken\":\"fixture-access\"}}"
    );
    Ok(())
}

#[tokio::test]
async fn s26_latest_resume_without_a_usable_candidate_skips_materialization() -> TestResult {
    let mut fixture = Fixture::new().await?;
    fixture.options = SessionStoreOptions::from(Arc::new(ListedStore {
        inner: fixture.store.clone(),
        sessions: vec![SessionListEntry {
            session_id: SessionId::from("not-a-uuid"),
            modified_at_ms: 1,
        }],
    }) as Arc<dyn SessionStore>);
    let runtime = fixture.prepare(StoreResume::Latest).await?;
    assert!(runtime.materialized.is_none());
    assert_eq!(runtime.resume_session_id, None);
    assert_eq!(runtime.mirror.projects_dir(), runtime.projects_dir);
    Ok(())
}

#[tokio::test]
async fn s22_a_non_uuid_resume_id_skips_materialization() -> TestResult {
    let fixture = Fixture::new().await?;
    let runtime = fixture
        .prepare(StoreResume::Session(SessionId::from("named-session")))
        .await?;
    assert!(runtime.materialized.is_none());
    assert_eq!(runtime.resume_session_id, None);
    assert_eq!(
        runtime.config_dir, None,
        "the child inherits the caller configuration directory"
    );
    assert_eq!(runtime.projects_dir, fixture.filesystem.path("config").join("projects"));
    assert!(fixture.filesystem.paths(Operation::CreateDirectory)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn s23_a_missing_or_empty_stored_session_skips_materialization() -> TestResult {
    let fixture = Fixture::new().await?;
    let empty = SessionId::from(uuid::Uuid::nil().to_string());
    fixture
        .store
        .append(&SessionKey::new(fixture.project.clone(), empty.clone())?, &[])
        .await?;
    for session in [SessionId::from("00000000-0000-4000-8000-000000000001"), empty] {
        let runtime = fixture.prepare(StoreResume::Session(session)).await?;
        assert!(runtime.materialized.is_none());
        assert_eq!(runtime.resume_session_id, None);
    }
    assert!(fixture.filesystem.paths(Operation::CreateDirectory)?.is_empty());
    Ok(())
}

struct DeclaredSubkeyStore {
    inner: Arc<InMemorySessionStore<FrozenClock>>,
    subkeys: Vec<String>,
}

impl SessionStore for DeclaredSubkeyStore {
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        self.inner.append(key, entries)
    }
    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        self.inner.load(key)
    }
    fn list_subkeys<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Vec<String>> {
        Box::pin(async { Ok(self.subkeys.clone()) })
    }
}

#[tokio::test]
async fn s24_unsafe_subkeys_from_the_store_are_skipped() -> TestResult {
    let mut fixture = Fixture::new().await?;
    fixture.options = SessionStoreOptions::from(Arc::new(DeclaredSubkeyStore {
        inner: fixture.store.clone(),
        subkeys: [
            "",
            "/absolute",
            "\\absolute",
            "../escape",
            "subagents/../escape",
            "subagents\\..\\escape",
            "./here",
            "C:drive",
            "nul\0byte",
            "subagents/agent-one",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    }) as Arc<dyn SessionStore>);
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    let project_dir = config.join("projects").join(fixture.project.as_str());
    assert!(
        fixture
            .filesystem
            .exists(&project_dir.join(SESSION).join("subagents/agent-one.jsonl"))
    );
    let created = fixture.filesystem.paths(Operation::CreateDirectory)?;
    assert!(created.iter().all(|path| path.starts_with(&config)));
    assert!(!fixture.filesystem.exists(&project_dir.join("escape")));
    Ok(())
}

struct ListedStore {
    inner: Arc<InMemorySessionStore<FrozenClock>>,
    sessions: Vec<SessionListEntry>,
}

impl SessionStore for ListedStore {
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        self.inner.append(key, entries)
    }
    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        self.inner.load(key)
    }
    fn list_sessions<'a>(&'a self, _: &'a SessionProjectKey) -> SessionStoreFuture<'a, Vec<SessionListEntry>> {
        Box::pin(async { Ok(self.sessions.clone()) })
    }
}

#[tokio::test]
async fn s26_latest_resume_walks_newest_first_past_unusable_candidates() -> TestResult {
    let mut fixture = Fixture::new().await?;
    let empty = "00000000-0000-4000-8000-000000000001";
    let sidechain = "00000000-0000-4000-8000-000000000002";
    let older = "00000000-0000-4000-8000-000000000003";
    let project = fixture.project.clone();
    let key = |id: &str| SessionKey::new(project.clone(), SessionId::from(id));
    fixture.store.append(&key(empty)?, &[]).await?;
    fixture
        .store
        .append(&key(sidechain)?, &[entry(json!({"type":"user","isSidechain":true}))?])
        .await?;
    fixture
        .store
        .append(&key(older)?, &[entry(json!({"type":"user","message":"older"}))?])
        .await?;
    let listed = |id: &str, modified_at_ms| SessionListEntry {
        session_id: SessionId::from(id),
        modified_at_ms,
    };
    fixture.options = SessionStoreOptions::from(Arc::new(ListedStore {
        inner: fixture.store.clone(),
        sessions: vec![
            listed(older, 10),
            listed(SESSION, 20),
            listed("not-a-uuid", 60),
            listed(empty, 50),
            listed(sidechain, 40),
            listed("00000000-0000-4000-8000-000000000004", 30),
        ],
    }) as Arc<dyn SessionStore>);
    let runtime = fixture.prepare(StoreResume::Latest).await?;
    assert_eq!(runtime.resume_session_id, Some(SessionId::from(SESSION)));
    Ok(())
}
