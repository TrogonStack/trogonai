use std::{
    ffi::{OsStr, OsString},
    fs::{DirBuilder, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

use tokio::time::timeout;

use crate::{
    Environment, SessionEntry, SessionError, SessionId, SessionKey, SessionProjectKey, SessionStoreOptions,
    SessionSubpath, TranscriptMirror,
    adapters::{
        EnvironmentVariable, NativeFileSystem, SessionFileSystem, SpawnProcess, SystemEnvironment, TemporaryDirectory,
        TokioProcessSpawner,
    },
    constants::{
        CLEANUP_ATTEMPTS, CLEANUP_DELAY, KEYCHAIN_FALLBACK_ACCOUNT, KEYCHAIN_SERVICE_NAME, KEYCHAIN_TIMEOUT,
        RESUME_SETTINGS_STRIPPED_KEYS,
    },
};

#[derive(Clone, Debug, Default)]
pub enum StoreResume {
    #[default]
    None,
    Latest,
    Session(SessionId),
}

/// Owns the temporary configuration directory a store-backed resume runs in.
///
/// [`MaterializedSession::cleanup`] removes it after the subprocess exits. Dropping the session without calling
/// `cleanup` makes one best-effort removal attempt.
pub struct MaterializedSession<F: SessionFileSystem = NativeFileSystem> {
    path: PathBuf,
    filesystem: F,
    removed: bool,
}

impl<F: SessionFileSystem> std::fmt::Debug for MaterializedSession<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaterializedSession").field("path", &self.path).finish()
    }
}

impl<F: SessionFileSystem> MaterializedSession<F> {
    pub fn config_dir(&self) -> &Path {
        &self.path
    }

    /// Removes the temporary configuration directory, retrying while another process briefly holds a handle on it.
    ///
    /// The lease is only marked removed once the work is finished, so cancelling this future mid-retry still lets
    /// `Drop` make its best-effort removal of the private tree.
    pub async fn cleanup(mut self) {
        for _ in 0..CLEANUP_ATTEMPTS {
            match self.filesystem.remove_dir_all(&self.path) {
                Ok(()) => {
                    self.removed = true;
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.removed = true;
                    return;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ResourceBusy
                            | io::ErrorKind::DirectoryNotEmpty
                            | io::ErrorKind::PermissionDenied
                    ) =>
                {
                    tokio::time::sleep(CLEANUP_DELAY).await;
                }
                Err(_) => break,
            }
        }
        if let Err(error) = self.filesystem.remove_dir_all(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "could not remove the resume configuration directory");
        }
        self.removed = true;
    }
}

impl<F: SessionFileSystem> Drop for MaterializedSession<F> {
    fn drop(&mut self) {
        if !self.removed {
            let _ = self.filesystem.remove_dir_all(&self.path);
        }
    }
}

pub struct SessionRuntime<F: SessionFileSystem = NativeFileSystem> {
    pub mirror: TranscriptMirror,
    pub materialized: Option<MaterializedSession<F>>,
    /// The `CLAUDE_CONFIG_DIR` the subprocess runs with, when the session runtime decides it.
    pub config_dir: Option<PathBuf>,
    pub projects_dir: PathBuf,
    pub resume_session_id: Option<SessionId>,
}

impl SessionRuntime {
    pub async fn prepare(
        options: &SessionStoreOptions,
        resume: StoreResume,
        cwd: &Path,
        subprocess_environment: &Environment,
    ) -> Result<Self, SessionError> {
        Self::prepare_with_adapters(SessionPreparationOptions {
            store_options: options,
            resume,
            cwd,
            subprocess_environment,
            environment: &SystemEnvironment,
            process: &TokioProcessSpawner,
            filesystem: NativeFileSystem,
        })
        .await
    }
}

pub struct SessionPreparationOptions<'a, E, P, F> {
    pub store_options: &'a SessionStoreOptions,
    pub resume: StoreResume,
    pub cwd: &'a Path,
    /// Environment overrides passed to the subprocess. Auth variables here count as configured auth.
    pub subprocess_environment: &'a Environment,
    pub environment: &'a E,
    pub process: &'a P,
    pub filesystem: F,
}

impl<F: SessionFileSystem> SessionRuntime<F> {
    pub async fn prepare_with_adapters<E, P>(
        input: SessionPreparationOptions<'_, E, P, F>,
    ) -> Result<Self, SessionError>
    where
        E: EnvironmentVariable + TemporaryDirectory,
        P: SpawnProcess<Error = io::Error>,
    {
        let SessionPreparationOptions {
            store_options: options,
            resume,
            cwd,
            subprocess_environment,
            environment,
            process,
            filesystem,
        } = input;
        let caller_config_dir = caller_config_dir(options, environment)?;
        let source_config_dir = match &caller_config_dir {
            Some(directory) => directory.clone(),
            None => home_dir(environment)?.join(".claude"),
        };
        let project = match &options.project_key {
            Some(key) => key.clone(),
            None => SessionProjectKey::for_directory_with_filesystem(cwd, &filesystem)?,
        };
        let resolved = match resume {
            StoreResume::None => None,
            StoreResume::Session(id) => {
                if is_uuid(&id) {
                    load_candidate(options, &project, id).await?
                } else {
                    tracing::debug!("resume id is not a UUID; passing it to the CLI without materializing");
                    None
                }
            }
            StoreResume::Latest => latest_candidate(options, &project).await?,
        };
        let Some((id, entries)) = resolved else {
            let projects_dir = source_config_dir.join("projects");
            return Ok(SessionRuntime {
                mirror: TranscriptMirror::new(options, projects_dir.clone()),
                materialized: None,
                config_dir: options.config_directory.clone(),
                projects_dir,
                resume_session_id: None,
            });
        };
        let path = environment
            .temp_dir()
            .join(format!("claude-resume-{}", uuid::Uuid::new_v4().simple()));
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        filesystem.create_directory(&builder, &path).map_err(SessionError::Io)?;
        let lease = MaterializedSession {
            path,
            filesystem,
            removed: false,
        };
        let materialization = Materialization {
            lease: &lease,
            store_options: options,
            project: &project,
            id: &id,
            budget: ResumeBudget::default(),
        };
        let auth = AuthSources {
            caller_config_dir: caller_config_dir.as_deref(),
            source_config_dir: &source_config_dir,
            subprocess_environment,
            environment,
            process,
            keychain: cfg!(target_os = "macos"),
        };
        if let Err(error) = materialization.run(entries, auth).await {
            lease.cleanup().await;
            return Err(error);
        }
        let config_dir = lease.path.clone();
        let projects_dir = config_dir.join("projects");
        Ok(SessionRuntime {
            mirror: TranscriptMirror::new(options, projects_dir.clone()),
            materialized: Some(lease),
            config_dir: Some(config_dir),
            projects_dir,
            resume_session_id: Some(id),
        })
    }
}

async fn load_candidate(
    options: &SessionStoreOptions,
    project: &SessionProjectKey,
    id: SessionId,
) -> Result<Option<(SessionId, Vec<SessionEntry>)>, SessionError> {
    let key = SessionKey::new(project.clone(), id.clone())?;
    let entries = timeout(options.limits.load_timeout.as_duration(), options.store.load(&key))
        .await
        .map_err(|_| SessionError::LoadTimeout)??;
    Ok(entries
        .filter(|entries| !entries.is_empty())
        .map(|entries| (id, entries)))
}

async fn latest_candidate(
    options: &SessionStoreOptions,
    project: &SessionProjectKey,
) -> Result<Option<(SessionId, Vec<SessionEntry>)>, SessionError> {
    let mut sessions = timeout(
        options.limits.load_timeout.as_duration(),
        options.store.list_sessions(project),
    )
    .await
    .map_err(|_| SessionError::LoadTimeout)??;
    if sessions.len() > options.limits.max_resume_entries.get() {
        return Err(SessionError::EntryLimit);
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified_at_ms));
    for session in sessions {
        if !is_uuid(&session.session_id) {
            continue;
        }
        let Some((id, entries)) = load_candidate(options, project, session.session_id).await? else {
            continue;
        };
        if entries.first().is_some_and(SessionEntry::is_sidechain) {
            continue;
        }
        return Ok(Some((id, entries)));
    }
    Ok(None)
}

fn is_uuid(id: &SessionId) -> bool {
    id.as_str().len() == 36 && uuid::Uuid::parse_str(id.as_str()).is_ok()
}

fn caller_config_dir<E: EnvironmentVariable>(
    options: &SessionStoreOptions,
    environment: &E,
) -> Result<Option<PathBuf>, SessionError> {
    let directory = options.config_directory.clone().or_else(|| {
        environment
            .var_os(OsStr::new("CLAUDE_CONFIG_DIR"))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    });
    match directory {
        Some(directory) if !directory.is_absolute() => Err(SessionError::UnsafeConfigPath),
        directory => Ok(directory),
    }
}

fn home_dir<E: EnvironmentVariable>(environment: &E) -> Result<PathBuf, SessionError> {
    let home = environment
        .var_os(OsStr::new(if cfg!(windows) { "USERPROFILE" } else { "HOME" }))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(SessionError::MissingConfigDirectory)?;
    if !home.is_absolute() {
        return Err(SessionError::UnsafeConfigPath);
    }
    Ok(home)
}

struct Materialization<'a, F: SessionFileSystem> {
    lease: &'a MaterializedSession<F>,
    store_options: &'a SessionStoreOptions,
    project: &'a SessionProjectKey,
    id: &'a SessionId,
    budget: ResumeBudget,
}

struct AuthSources<'a, E, P> {
    caller_config_dir: Option<&'a Path>,
    source_config_dir: &'a Path,
    subprocess_environment: &'a Environment,
    environment: &'a E,
    process: &'a P,
    /// Whether the platform keeps OAuth credentials in a keychain this process can read.
    keychain: bool,
}

impl<F: SessionFileSystem> Materialization<'_, F> {
    async fn run<E, P>(mut self, entries: Vec<SessionEntry>, auth: AuthSources<'_, E, P>) -> Result<(), SessionError>
    where
        E: EnvironmentVariable,
        P: SpawnProcess<Error = io::Error>,
    {
        let filesystem = &self.lease.filesystem;
        let project_dir = self.lease.path.join("projects").join(self.project.as_str());
        create_directories(filesystem, &project_dir)?;
        let contents = encode_entries(&entries, self.store_options, &mut self.budget)?;
        write_private(
            filesystem,
            &project_dir.join(format!("{}.jsonl", self.id.as_str())),
            &contents,
        )?;
        copy_auth_files(filesystem, &self.lease.path, auth).await?;
        self.materialize_subkeys(&project_dir).await
    }

    async fn materialize_subkeys(&mut self, project_dir: &Path) -> Result<(), SessionError> {
        let options = self.store_options;
        let key = SessionKey::new(self.project.clone(), self.id.clone())?;
        let subpaths = timeout(
            options.limits.load_timeout.as_duration(),
            options.store.list_subkeys(&key),
        )
        .await
        .map_err(|_| SessionError::LoadTimeout)??;
        if subpaths.len() > options.limits.max_subkeys.get() {
            return Err(SessionError::SubpathLimit);
        }
        let session_dir = project_dir.join(self.id.as_str());
        for subpath in subpaths {
            let subpath = match SessionSubpath::new(subpath) {
                Ok(subpath) => subpath,
                Err(error) => {
                    tracing::warn!(%error, "skipping unsafe subpath from list_subkeys");
                    continue;
                }
            };
            let subkey = key.clone().with_subpath(subpath.clone());
            let entries = timeout(options.limits.load_timeout.as_duration(), options.store.load(&subkey))
                .await
                .map_err(|_| SessionError::LoadTimeout)??;
            let Some(entries) = entries.filter(|entries| !entries.is_empty()) else {
                continue;
            };
            self.write_subpath(&session_dir, &subpath, entries)?;
        }
        Ok(())
    }

    fn write_subpath(
        &mut self,
        session_dir: &Path,
        subpath: &SessionSubpath,
        entries: Vec<SessionEntry>,
    ) -> Result<(), SessionError> {
        let options = self.store_options;
        let filesystem = &self.lease.filesystem;
        let target = session_dir.join(subpath.as_str());
        let directory = target.parent().unwrap_or(session_dir);
        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let metadata = entries.iter().rev().find(|entry| entry.kind() == "agent_metadata");
        let transcript: Vec<_> = entries
            .iter()
            .filter(|entry| entry.kind() != "agent_metadata")
            .cloned()
            .collect();
        create_directories(filesystem, directory)?;
        if !transcript.is_empty() {
            let contents = encode_entries(&transcript, options, &mut self.budget)?;
            write_private(filesystem, &directory.join(format!("{name}.jsonl")), &contents)?;
        }
        if let Some(metadata) = metadata {
            let mut object = metadata.as_object().clone();
            object.remove("type");
            let mut writer = ResumeWriter::new(options.limits.max_resume_bytes.get().saturating_sub(self.budget.bytes));
            writer.encode(&object)?;
            let contents = writer.bytes;
            self.budget.entries = self.budget.entries.saturating_add(1);
            self.budget.bytes = self.budget.bytes.saturating_add(contents.len());
            if self.budget.entries > options.limits.max_resume_entries.get() {
                return Err(SessionError::EntryLimit);
            }
            write_private(filesystem, &directory.join(format!("{name}.meta.json")), &contents)?;
        }
        Ok(())
    }
}

fn create_directories<F: SessionFileSystem>(filesystem: &F, path: &Path) -> Result<(), SessionError> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    filesystem.create_directory(&builder, path).map_err(SessionError::Io)
}

fn write_private<F: SessionFileSystem>(filesystem: &F, path: &Path, contents: &[u8]) -> Result<(), SessionError> {
    write_private_file(filesystem, path, contents).map_err(SessionError::Io)
}

fn write_private_file<F: SessionFileSystem>(filesystem: &F, path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = filesystem.open(&options, path)?;
    file.write_all(contents)
}

async fn copy_auth_files<F, E, P>(
    filesystem: &F,
    config_dir: &Path,
    auth: AuthSources<'_, E, P>,
) -> Result<(), SessionError>
where
    F: SessionFileSystem,
    E: EnvironmentVariable,
    P: SpawnProcess<Error = io::Error>,
{
    let AuthSources {
        caller_config_dir,
        source_config_dir,
        subprocess_environment,
        environment,
        process,
        keychain,
    } = auth;
    let mut credentials = read_if_present(filesystem, &source_config_dir.join(".credentials.json"));
    let has_auth_variable = |name: &str| {
        subprocess_environment
            .get(name)
            .is_some_and(|value| !value.expose().is_empty())
            || environment
                .var_os(OsStr::new(name))
                .is_some_and(|value| !value.is_empty())
    };
    if keychain
        && caller_config_dir.is_none()
        && !has_auth_variable("ANTHROPIC_API_KEY")
        && !has_auth_variable("CLAUDE_CODE_OAUTH_TOKEN")
        && let Some(keychain) = read_keychain_credentials(environment, process).await
    {
        credentials = Some(keychain);
    }
    if let Some(credentials) = credentials {
        write_private(
            filesystem,
            &config_dir.join(".credentials.json"),
            &redact_refresh_token(credentials),
        )?;
    }
    let claude_json = match caller_config_dir {
        Some(directory) => directory.join(".claude.json"),
        None => home_dir(environment)?.join(".claude.json"),
    };
    copy_if_present(filesystem, &claude_json, &config_dir.join(".claude.json"), None);
    for name in ["settings.json", "cowork_settings.json"] {
        copy_if_present(
            filesystem,
            &source_config_dir.join(name),
            &config_dir.join(name),
            Some(strip_settings_for_resume),
        );
    }
    Ok(())
}

fn read_if_present<F: SessionFileSystem>(filesystem: &F, source: &Path) -> Option<Vec<u8>> {
    let read = || -> io::Result<Vec<u8>> {
        if !filesystem.metadata(source)?.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        let mut contents = Vec::new();
        filesystem
            .open(OpenOptions::new().read(true), source)?
            .read_to_end(&mut contents)?;
        Ok(contents)
    };
    match read() {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(path = %source.display(), %error, "resume: skipping unreadable configuration file");
            None
        }
    }
}

fn copy_if_present<F: SessionFileSystem>(
    filesystem: &F,
    source: &Path,
    destination: &Path,
    transform: Option<fn(Vec<u8>) -> Vec<u8>>,
) {
    let Some(contents) = read_if_present(filesystem, source) else {
        return;
    };
    let contents = match transform {
        Some(transform) => transform(contents),
        None => contents,
    };
    if let Err(error) = write_private_file(filesystem, destination, &contents) {
        let _ = filesystem.remove_file(destination);
        tracing::warn!(path = %source.display(), %error, "resume: skipping configuration file that could not be copied");
    }
}

fn redact_refresh_token(credentials: Vec<u8>) -> Vec<u8> {
    let Ok(mut parsed) = serde_json::from_slice::<serde_json::Value>(&credentials) else {
        return credentials;
    };
    let removed = parsed
        .get_mut("claudeAiOauth")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|oauth| oauth.remove("refreshToken"))
        .is_some();
    if !removed {
        return credentials;
    }
    serde_json::to_vec(&parsed).unwrap_or(credentials)
}

fn strip_settings_for_resume(contents: Vec<u8>) -> Vec<u8> {
    let text = contents.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&contents);
    let Ok(serde_json::Value::Object(mut parsed)) = serde_json::from_slice::<serde_json::Value>(text) else {
        return contents;
    };
    let mut stripped = false;
    for key in RESUME_SETTINGS_STRIPPED_KEYS {
        stripped |= parsed.remove(key).is_some();
    }
    if let Some(serde_json::Value::Object(env)) = parsed.get_mut("env") {
        stripped |= env.remove("CLAUDE_CONFIG_DIR").is_some();
    }
    if !stripped {
        return contents;
    }
    serde_json::to_vec(&parsed).unwrap_or(contents)
}

/// Reads OAuth credentials from the macOS Keychain. Returns `None` on any failure.
async fn read_keychain_credentials<E, P>(environment: &E, process: &P) -> Option<Vec<u8>>
where
    E: EnvironmentVariable,
    P: SpawnProcess<Error = io::Error>,
{
    let account = environment
        .var_os(OsStr::new("USER"))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from(KEYCHAIN_FALLBACK_ACCOUNT));
    let mut command = tokio::process::Command::new("security");
    command
        .arg("find-generic-password")
        .arg("-a")
        .arg(account)
        .args(["-w", "-s", KEYCHAIN_SERVICE_NAME])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = process.spawn(&mut command).ok()?;
    let output = timeout(KEYCHAIN_TIMEOUT, child.wait_with_output()).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.as_bytes().to_vec())
}

#[derive(Default)]
struct ResumeBudget {
    entries: usize,
    bytes: usize,
}

fn encode_entries(
    entries: &[SessionEntry],
    options: &SessionStoreOptions,
    budget: &mut ResumeBudget,
) -> Result<Vec<u8>, SessionError> {
    budget.entries = budget.entries.saturating_add(entries.len());
    if budget.entries > options.limits.max_resume_entries.get() {
        return Err(SessionError::EntryLimit);
    }
    let mut writer = ResumeWriter::new(options.limits.max_resume_bytes.get().saturating_sub(budget.bytes));
    for entry in entries {
        writer.encode(entry.as_object())?;
        writer.write_all(b"\n").map_err(|_| SessionError::ByteLimit)?;
    }
    let bytes = writer.bytes;
    budget.bytes = budget.bytes.saturating_add(bytes.len());
    Ok(bytes)
}

struct ResumeWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl ResumeWriter {
    fn encode<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> Result<(), SessionError> {
        serde_json::to_writer(&mut *self, value).map_err(|error| {
            if self.exceeded {
                SessionError::ByteLimit
            } else {
                SessionError::Encoding(error)
            }
        })
    }

    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}

impl Write for ResumeWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::from(std::io::ErrorKind::FileTooLarge));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "session_resume_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "session_resume_keychain_tests.rs"]
mod keychain_tests;
