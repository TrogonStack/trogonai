#![cfg(all(unix, feature = "test-support"))]
//! Home directory lookups of the resume source directory, per upstream dialect: Node `os.homedir()` for TypeScript
//! and Python `Path.home()`. Both read the process environment and fall back to the passwd database of the user.

#[path = "support/recording_filesystem.rs"]
mod recording_filesystem;

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use recording_filesystem::RecordingFileSystem;
use serde_json::json;
use trogon_claude_agent_sdk::{
    Environment, EnvironmentMode, InMemorySessionStore, SessionEntry, SessionError, SessionId, SessionKey,
    SessionPreparationOptions, SessionProjectKey, SessionRuntime, SessionStore, SessionStoreOptions, StoreResume,
    SummaryDialect,
    adapters::{SpawnProcess, WallClock},
    test_support::{FixedEnvironment, WithPasswd},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";
const MARKER: &[u8] = b"{\"marker\":true}";
const DIALECTS: [SummaryDialect; 2] = [SummaryDialect::TypeScript, SummaryDialect::Python];

struct FrozenClock;
impl WallClock for FrozenClock {
    type Time = SystemTime;
    fn now(&self) -> SystemTime {
        UNIX_EPOCH
    }
}

/// Refuses every spawn, so no test reads a real keychain.
struct NoProcess;
impl SpawnProcess for NoProcess {
    type Error = io::Error;
    fn spawn(&self, _: &mut tokio::process::Command) -> io::Result<tokio::process::Child> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// One resume with no `CLAUDE_CONFIG_DIR`, the given process `HOME` and the given passwd home for both users.
struct Scenario {
    filesystem: RecordingFileSystem,
    dialect: SummaryDialect,
    home: Option<OsString>,
    passwd: Option<PathBuf>,
    require_absolute: bool,
}

impl Scenario {
    fn new(dialect: SummaryDialect) -> io::Result<Self> {
        let filesystem = RecordingFileSystem::new()?;
        filesystem.create_dir_all(&filesystem.path("project"))?;
        filesystem.create_dir_all(&filesystem.path("tmp"))?;
        Ok(Self {
            filesystem,
            dialect,
            home: None,
            passwd: None,
            require_absolute: false,
        })
    }

    fn with_home(mut self, home: impl Into<OsString>) -> Self {
        self.home = Some(home.into());
        self
    }

    fn with_passwd(mut self, passwd: PathBuf) -> Self {
        self.passwd = Some(passwd);
        self
    }

    fn requiring_absolute_paths(mut self) -> Self {
        self.require_absolute = true;
        self
    }

    /// Puts the marker where a source directory `<home>/.claude` would hold it.
    fn plant(&self, home: &Path) -> io::Result<()> {
        self.filesystem
            .write_file(&home.join(".claude/cowork_settings.json"), MARKER)
    }

    /// Resumes and returns the marker the materialized directory received from the source directory, if any.
    async fn resume(&self) -> Result<Option<Vec<u8>>, SessionError> {
        let mut variables = BTreeMap::from([(OsString::from("TMPDIR"), self.filesystem.path("tmp").into_os_string())]);
        if let Some(home) = &self.home {
            variables.insert(OsString::from("HOME"), home.clone());
        }
        let environment = WithPasswd {
            environment: FixedEnvironment {
                directory: self.filesystem.path("project"),
                variables,
            },
            effective: self.passwd.clone().map(PathBuf::into_os_string),
            real: self.passwd.clone().map(PathBuf::into_os_string),
        };
        let project = SessionProjectKey::new("project").map_err(|_| SessionError::MissingConfigDirectory)?;
        let store = Arc::new(InMemorySessionStore::with_clock(FrozenClock));
        let key = SessionKey::new(project.clone(), SessionId::from(SESSION))
            .map_err(|_| SessionError::MissingConfigDirectory)?;
        let entry = SessionEntry::try_from(json!({"type":"user","message":"hello"}))
            .map_err(|_| SessionError::MissingConfigDirectory)?;
        store
            .append(&key, &[entry])
            .await
            .map_err(|_| SessionError::MissingConfigDirectory)?;
        let mut options = SessionStoreOptions::from(store as Arc<dyn SessionStore>);
        options.dialect = self.dialect;
        options.project_key = Some(project);
        options.require_absolute_config_paths = self.require_absolute;
        let runtime = SessionRuntime::prepare_with_adapters(SessionPreparationOptions {
            store_options: &options,
            resume: StoreResume::Session(SessionId::from(SESSION)),
            cwd: &environment.environment.directory,
            subprocess_environment: &Environment::default(),
            environment_mode: EnvironmentMode::default(),
            environment: &environment,
            process: &NoProcess,
            filesystem: self.filesystem.clone(),
        })
        .await?;
        Ok(runtime
            .config_dir
            .as_ref()
            .and_then(|config| self.filesystem.read(&config.join("cowork_settings.json")).ok()))
    }
}

#[tokio::test]
async fn an_unset_home_resolves_to_the_passwd_home_in_both_dialects() -> TestResult {
    for dialect in DIALECTS {
        let scenario = Scenario::new(dialect)?;
        let passwd = scenario.filesystem.path("passwd-home");
        scenario.plant(&passwd)?;
        let scenario = scenario.with_passwd(passwd);
        assert_eq!(scenario.resume().await?, Some(MARKER.to_vec()), "{dialect:?}");
    }
    Ok(())
}

#[tokio::test]
async fn the_process_home_wins_over_the_passwd_home() -> TestResult {
    for dialect in DIALECTS {
        let scenario = Scenario::new(dialect)?;
        let home = scenario.filesystem.path("env-home");
        scenario.plant(&home)?;
        let scenario = scenario
            .with_home(home.into_os_string())
            .with_passwd(PathBuf::from("/nonexistent-passwd-home"));
        assert_eq!(scenario.resume().await?, Some(MARKER.to_vec()), "{dialect:?}");
    }
    Ok(())
}

#[tokio::test]
async fn an_empty_home_is_used_as_set_and_never_falls_back_to_the_passwd_home() -> TestResult {
    for dialect in DIALECTS {
        let scenario = Scenario::new(dialect)?;
        let passwd = scenario.filesystem.path("passwd-home");
        scenario.plant(&passwd)?;
        let scenario = scenario.with_home("").with_passwd(passwd);
        assert_eq!(scenario.resume().await?, None, "{dialect:?}");
    }
    Ok(())
}

#[tokio::test]
async fn no_home_in_the_environment_or_the_passwd_database_is_a_missing_config_directory() -> TestResult {
    for dialect in DIALECTS {
        let scenario = Scenario::new(dialect)?;
        assert!(
            matches!(scenario.resume().await, Err(SessionError::MissingConfigDirectory)),
            "{dialect:?}"
        );
    }
    Ok(())
}

/// Node `path.join` resolves `..` and repeated separators, while Python `Path.home()` only collapses the
/// separators and keeps `..`, which the operating system then resolves through a directory that does not exist.
#[tokio::test]
async fn a_messy_home_is_normalized_by_node_but_not_by_python() -> TestResult {
    for (dialect, found) in [(SummaryDialect::TypeScript, true), (SummaryDialect::Python, false)] {
        let scenario = Scenario::new(dialect)?;
        let root = scenario.filesystem.path("");
        scenario.plant(&root.join("h/b"))?;
        let messy = format!("{}/h//a/../b/", root.display());
        let scenario = scenario.with_home(messy);
        let expected = found.then(|| MARKER.to_vec());
        assert_eq!(scenario.resume().await?, expected, "{dialect:?}");
    }
    Ok(())
}

/// Python `expanduser("~")` leaves a home that starts with `~` unexpanded, so `Path.home()` raises; Node joins it
/// as an ordinary relative directory.
#[tokio::test]
async fn a_home_starting_with_a_tilde_fails_only_in_python() -> TestResult {
    let typescript = Scenario::new(SummaryDialect::TypeScript)?.with_home("~x");
    assert_eq!(typescript.resume().await?, None);
    let python = Scenario::new(SummaryDialect::Python)?.with_home("~x");
    assert!(matches!(
        python.resume().await,
        Err(SessionError::MissingConfigDirectory)
    ));
    Ok(())
}

#[tokio::test]
async fn the_absolute_path_check_stays_opt_in_for_a_relative_home() -> TestResult {
    for dialect in DIALECTS {
        let relaxed = Scenario::new(dialect)?.with_home("relative/home");
        assert_eq!(relaxed.resume().await?, None, "{dialect:?}");
        let strict = Scenario::new(dialect)?
            .with_home("relative/home")
            .requiring_absolute_paths();
        assert!(
            matches!(strict.resume().await, Err(SessionError::UnsafeConfigPath)),
            "{dialect:?}"
        );
    }
    Ok(())
}
