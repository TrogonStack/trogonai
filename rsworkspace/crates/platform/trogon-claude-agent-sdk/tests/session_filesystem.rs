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
    Environment, EnvironmentMode, InMemorySessionStore, SessionEntry, SessionError, SessionId, SessionKey,
    SessionListEntry, SessionPreparationOptions, SessionProjectKey, SessionRuntime, SessionStore, SessionStoreFuture,
    SessionStoreOperation, SessionStoreOptions, SessionSubpath, StoreResume, SummaryDialect,
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
        let key = SessionKey::new(project.clone(), SessionId::from(SESSION));
        let store = Arc::new(InMemorySessionStore::with_clock(FrozenClock));
        store
            .append(&key, &[entry(json!({"type":"user","message":"hello"}))?])
            .await?;
        store
            .append(
                &key.with_subpath(SessionSubpath::new("subagents/agent-one")),
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
            environment_mode: EnvironmentMode::default(),
            environment: &self.environment,
            process: &self.process,
            filesystem: self.filesystem.clone(),
        })
        .await
    }

    /// Switches the fixture to the Python SDK's resume rules; the TypeScript rules are the default.
    fn python(mut self) -> Self {
        self.options.dialect = SummaryDialect::Python;
        self
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
    let fixture = Fixture::new().await?.python();
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
    let fixture = Fixture::new().await?.python();
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
async fn w3_a_relative_home_directory_is_used_as_given_unless_absolute_paths_are_required() -> TestResult {
    let mut fixture = Fixture::new().await?.python();
    fixture.environment.variables.remove(OsStr::new("CLAUDE_CONFIG_DIR"));
    fixture
        .environment
        .variables
        .insert(OsString::from("HOME"), OsString::from("relative/home"));
    let runtime = fixture.resume().await?;
    assert!(runtime.materialized.is_some());
    drop(runtime);
    fixture.options.require_absolute_config_paths = true;
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
    let fixture = Fixture::new().await?.python();
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
        .append(&SessionKey::new(fixture.project.clone(), empty.clone()), &[])
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
    fn supports(&self, operation: SessionStoreOperation) -> bool {
        matches!(
            operation,
            SessionStoreOperation::Append | SessionStoreOperation::Load | SessionStoreOperation::ListSubkeys
        )
    }
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

/// Lists one subkey, whatever its spelling, and serves the same transcript for it.
struct DotSubkeyStore {
    inner: Arc<InMemorySessionStore<FrozenClock>>,
    subkey: String,
    transcript: Vec<SessionEntry>,
}

impl SessionStore for DotSubkeyStore {
    fn supports(&self, operation: SessionStoreOperation) -> bool {
        matches!(
            operation,
            SessionStoreOperation::Append | SessionStoreOperation::Load | SessionStoreOperation::ListSubkeys
        )
    }
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        self.inner.append(key, entries)
    }
    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        if key.subpath().is_some() {
            let transcript = self.transcript.clone();
            return Box::pin(async move { Ok(Some(transcript)) });
        }
        self.inner.load(key)
    }
    fn list_subkeys<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Vec<String>> {
        Box::pin(async { Ok(vec![self.subkey.clone()]) })
    }
}

/// TypeScript `Jtt` keeps a subpath with a `.` component because its transcript still lands inside the session
/// directory, and writes it where Node's `path.join` puts it. Python's `_is_safe_subpath` skips it.
#[tokio::test]
async fn s24_ts_a_dot_component_subpath_is_written_where_typescript_writes_it() -> TestResult {
    for python in [false, true] {
        let mut fixture = Fixture::new().await?;
        fixture.options = SessionStoreOptions::from(Arc::new(DotSubkeyStore {
            inner: fixture.store.clone(),
            subkey: "./subagents/agent-dot".to_owned(),
            transcript: vec![entry(json!({"type":"user","message":"from the dot subkey"}))?],
        }) as Arc<dyn SessionStore>);
        if python {
            fixture = fixture.python();
        }
        let runtime = fixture.resume().await?;
        let config = runtime.config_dir.clone().ok_or("missing config directory")?;
        let session_dir = config.join("projects").join(fixture.project.as_str()).join(SESSION);
        assert_eq!(
            fixture
                .filesystem
                .exists(&session_dir.join("subagents/agent-dot.jsonl")),
            !python
        );
    }
    Ok(())
}

const RESUME_GOLDENS: &str = include_str!("fixtures/session_key/resume.json");

/// Every file below `root`, relative to it, in sorted order; none when `root` does not exist.
fn files_below(root: &Path) -> io::Result<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(
                    path.strip_prefix(root)
                        .map_err(io::Error::other)?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Goldens captured from the real TypeScript `Jtt` and Python `_materialize_subkeys` with the same store: which
/// transcript file each listed subpath produces, if any, or whether the write fails.
#[tokio::test]
async fn s24_listed_subpaths_are_written_or_skipped_as_each_upstream_sdk_does() -> TestResult {
    let goldens: Value = serde_json::from_str(RESUME_GOLDENS)?;
    for case in goldens["subpaths"].as_array().ok_or("missing subpaths")? {
        let subpath = case["subpath"].as_str().ok_or("missing subpath")?;
        for (dialect, python) in [("typescript", false), ("python", true)] {
            let expected: Vec<&str> = case[dialect]
                .as_array()
                .ok_or("missing files")?
                .iter()
                .map(|file| file.as_str().ok_or("file is not text"))
                .collect::<Result<_, _>>()?;
            let fails = case[format!("{dialect}_write_fails")]
                .as_bool()
                .ok_or("missing failure flag")?;
            let mut fixture = Fixture::new().await?;
            fixture.options = SessionStoreOptions::from(Arc::new(DotSubkeyStore {
                inner: fixture.store.clone(),
                subkey: subpath.to_owned(),
                transcript: vec![entry(json!({"type":"user","message":"listed"}))?],
            }) as Arc<dyn SessionStore>);
            if python {
                fixture = fixture.python();
            }
            let outcome = fixture.resume().await;
            if fails {
                assert!(outcome.is_err(), "{dialect} {subpath:?} should fail to write");
                assert!(expected.is_empty());
                continue;
            }
            let runtime = outcome?;
            let config = runtime.config_dir.clone().ok_or("missing config directory")?;
            let session_dir = config.join("projects").join(fixture.project.as_str()).join(SESSION);
            assert_eq!(files_below(&session_dir)?, expected, "{dialect} {subpath:?}");
        }
    }
    Ok(())
}

/// Goldens captured from the real TypeScript `Qe` and Python `_validate_uuid`: a resume id that fails the check is
/// passed to the CLI without materializing, even when the store holds a session under that id.
#[tokio::test]
async fn s22_resume_ids_are_checked_as_each_upstream_sdk_checks_them() -> TestResult {
    let goldens: Value = serde_json::from_str(RESUME_GOLDENS)?;
    for case in goldens["session_ids"].as_array().ok_or("missing session ids")? {
        let id = case["session_id"].as_str().ok_or("missing id")?;
        for (dialect, python) in [("typescript", false), ("python", true)] {
            let accepted = case[dialect].as_bool().ok_or("missing verdict")?;
            let mut fixture = Fixture::new().await?;
            fixture
                .store
                .append(
                    &SessionKey::new(fixture.project.clone(), SessionId::from(id)),
                    &[entry(json!({"type":"user","message":"stored"}))?],
                )
                .await?;
            if python {
                fixture = fixture.python();
            }
            let runtime = fixture.prepare(StoreResume::Session(SessionId::from(id))).await?;
            assert_eq!(runtime.materialized.is_some(), accepted, "{dialect} {id:?}");
        }
    }
    Ok(())
}

struct ListedStore {
    inner: Arc<InMemorySessionStore<FrozenClock>>,
    sessions: Vec<SessionListEntry>,
}

impl SessionStore for ListedStore {
    fn supports(&self, operation: SessionStoreOperation) -> bool {
        matches!(
            operation,
            SessionStoreOperation::Append | SessionStoreOperation::Load | SessionStoreOperation::List
        )
    }
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
    fixture.options.dialect = SummaryDialect::Python;
    let runtime = fixture.prepare(StoreResume::Latest).await?;
    assert_eq!(runtime.resume_session_id, Some(SessionId::from(SESSION)));
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// TypeScript `A7` creates the resume root and project directory `0700`, writes the transcript and credentials `0600`
/// and rewritten settings `0600`, copies `.claude.json`, `.config.json` and the OAuth files with `fs.copyFile` (so
/// they keep the source permission bits) and creates subagent directories with the default mode.
#[tokio::test]
async fn w3_ts_resume_permission_bits_follow_the_bundle() -> TestResult {
    let fixture = Fixture::new().await?;
    let source = fixture.filesystem.path("config");
    fixture.filesystem.write_file(
        &source.join(".credentials.json"),
        b"{\"claudeAiOauth\":{\"refreshToken\":\"fixture-refresh\",\"accessToken\":\"fixture-access\"}}",
    )?;
    fixture.filesystem.write_file(
        &source.join("settings.json"),
        b"{\"enabledPlugins\":{},\"model\":\"fixture\"}",
    )?;
    fixture
        .filesystem
        .write_file(&source.join("cowork_settings.json"), b"{\"untouched\":true}")?;
    let copied = [
        (".claude.json", 0o644),
        (".config.json", 0o640),
        (".claude-staging-oauth.json", 0o604),
        (".claude-local-oauth.json", 0o664),
        (".claude-custom-oauth.json", 0o400),
    ];
    for (name, permissions) in copied {
        let path = source.join(name);
        fixture.filesystem.write_file(&path, name.as_bytes())?;
        set_mode(&path, permissions)?;
    }
    let reference = fixture.filesystem.path("default-mode");
    std::fs::create_dir(&reference)?;
    let runtime = fixture.resume().await?;
    let config = runtime.config_dir.clone().ok_or("missing config directory")?;
    let project_dir = config.join("projects").join(fixture.project.as_str());
    assert_eq!(mode(&config)?, 0o700);
    assert_eq!(mode(&config.join("projects"))?, 0o700);
    assert_eq!(mode(&project_dir)?, 0o700);
    assert_eq!(mode(&project_dir.join(format!("{SESSION}.jsonl")))?, 0o600);
    for directory in [project_dir.join(SESSION), project_dir.join(SESSION).join("subagents")] {
        assert_eq!(mode(&directory)?, mode(&reference)?);
    }
    for file in ["subagents/agent-one.jsonl", "subagents/agent-one.meta.json"] {
        assert_eq!(mode(&project_dir.join(SESSION).join(file))?, 0o600);
    }
    for name in [".credentials.json", "settings.json", "cowork_settings.json"] {
        assert_eq!(mode(&config.join(name))?, 0o600, "{name}");
    }
    assert_eq!(
        fixture.filesystem.read(&config.join(".credentials.json"))?,
        b"{\"claudeAiOauth\":{\"accessToken\":\"fixture-access\"}}"
    );
    assert_eq!(
        fixture.filesystem.read(&config.join("settings.json"))?,
        b"{\"model\":\"fixture\"}"
    );
    for (name, permissions) in copied {
        assert_eq!(fixture.filesystem.read(&config.join(name))?, name.as_bytes());
        assert_eq!(mode(&config.join(name))?, permissions, "{name}");
    }
    for (name, _) in copied {
        set_mode(&source.join(name), 0o600)?;
    }
    Ok(())
}

/// TypeScript reads `.credentials.json` with `readFile` and rethrows everything but `ENOENT`, so an unreadable file or
/// a directory in its place fails the resume and the partial directory is removed.
#[tokio::test]
async fn w3_ts_unreadable_credentials_fail_the_resume() -> TestResult {
    let fixture = Fixture::new().await?;
    let source = fixture.filesystem.path("config");
    fixture
        .filesystem
        .write_file(&source.join(".credentials.json"), b"{}")?;
    fixture.filesystem.fail(
        Operation::Metadata,
        Some("config/.credentials.json"),
        io::ErrorKind::PermissionDenied,
        1,
    )?;
    assert!(
        matches!(fixture.resume().await, Err(SessionError::Io(source)) if source.kind() == io::ErrorKind::PermissionDenied)
    );
    assert!(fixture.temporary_entries()?.is_empty());
    std::fs::remove_file(source.join(".credentials.json"))?;
    fixture.filesystem.create_dir_all(&source.join(".credentials.json"))?;
    assert!(matches!(fixture.resume().await, Err(SessionError::Io(_))));
    assert!(fixture.temporary_entries()?.is_empty());
    Ok(())
}

/// TypeScript joins a relative home directory with `path.join` and does not reject it.
#[tokio::test]
async fn w3_ts_a_relative_home_directory_is_used_as_given() -> TestResult {
    let mut fixture = Fixture::new().await?;
    fixture.environment.variables.remove(OsStr::new("CLAUDE_CONFIG_DIR"));
    fixture
        .environment
        .variables
        .insert(OsString::from("HOME"), OsString::from("relative/home"));
    let runtime = fixture.resume().await?;
    assert!(runtime.materialized.is_some());
    assert_eq!(runtime.resume_session_id, Some(SessionId::from(SESSION)));
    Ok(())
}

/// TypeScript continue (`nnt`) takes the newest listed session as is: no UUID walk and no sidechain skip.
#[tokio::test]
async fn s26_ts_latest_resume_takes_only_the_newest_listed_session() -> TestResult {
    let mut fixture = Fixture::new().await?;
    let sidechain = SessionId::from(uuid::Uuid::from_u128(2).to_string());
    let key = SessionKey::new(fixture.project.clone(), sidechain.clone());
    fixture
        .store
        .append(&key, &[entry(json!({"type":"user","isSidechain":true}))?])
        .await?;
    let listed = |id: &SessionId, modified_at_ms| SessionListEntry {
        session_id: id.clone(),
        modified_at_ms,
    };
    let session = SessionId::from(SESSION);
    let named = SessionId::from("not-a-uuid");
    for (sessions, expected) in [
        (
            vec![listed(&session, 10), listed(&sidechain, 20)],
            Some(sidechain.clone()),
        ),
        (vec![listed(&session, 10), listed(&named, 20)], None),
        (
            vec![
                listed(&session, 10),
                listed(&SessionId::from(uuid::Uuid::from_u128(3).to_string()), 20),
            ],
            None,
        ),
    ] {
        fixture.options = SessionStoreOptions::from(Arc::new(ListedStore {
            inner: fixture.store.clone(),
            sessions,
        }) as Arc<dyn SessionStore>);
        let runtime = fixture.prepare(StoreResume::Latest).await?;
        assert_eq!(runtime.resume_session_id, expected);
    }
    fixture.options = SessionStoreOptions::from(Arc::new(DeclaredSubkeyStore {
        inner: fixture.store.clone(),
        subkeys: Vec::new(),
    }) as Arc<dyn SessionStore>);
    assert!(matches!(
        fixture.prepare(StoreResume::Latest).await,
        Err(SessionError::ContinueRequiresList)
    ));
    Ok(())
}
