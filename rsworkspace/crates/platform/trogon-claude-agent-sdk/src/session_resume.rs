use std::{
    ffi::{OsStr, OsString},
    fs::{DirBuilder, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

use sha2::{Digest, Sha256};
use tokio::time::timeout;
use unicode_normalization::UnicodeNormalization;

use crate::{
    Environment, EnvironmentMode, JsonObject, SessionEntry, SessionError, SessionId, SessionKey, SessionProjectKey,
    SessionProjectKeyError, SessionStoreOperation, SessionStoreOptions, SessionSubpath, SummaryDialect,
    TranscriptMirror,
    adapters::{
        EnvironmentVariable, NativeFileSystem, SessionFileSystem, SpawnProcess, SystemEnvironment, TemporaryDirectory,
        TokioProcessSpawner,
    },
    constants::{
        APPROVED_OAUTH_ENDPOINTS_TS, CLEANUP_ATTEMPTS, CLEANUP_DELAY, KEYCHAIN_FALLBACK_ACCOUNT, KEYCHAIN_SERVICE_NAME,
        KEYCHAIN_TIMEOUT, OAUTH_FILE_SUFFIXES_TS, RESUME_SETTINGS_STRIPPED_KEYS, RESUME_SETTINGS_STRIPPED_KEYS_TS,
    },
    js_object_order::js_ordered,
    session_project_key::lexical_resolve,
    settings_json::js_stringify,
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
        environment_mode: EnvironmentMode,
    ) -> Result<Self, SessionError> {
        Self::prepare_with_adapters(SessionPreparationOptions {
            store_options: options,
            resume,
            cwd,
            subprocess_environment,
            environment_mode,
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
    /// How the subprocess environment relates to this process's: `Replace` is TypeScript's `options.env`, which
    /// stands in for the process environment; the other modes consult both, as the subprocess inherits them.
    pub environment_mode: EnvironmentMode,
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
            environment_mode,
            environment,
            process,
            filesystem,
        } = input;
        let environment_mode = environment_mode.resolve(subprocess_environment);
        let ts = options.dialect == SummaryDialect::TypeScript;
        let caller_config_dir = caller_config_dir(options, environment, ts)?;
        let source_config_dir = match &caller_config_dir {
            Some(directory) => directory.clone(),
            None => home_dir(environment, options.require_absolute_config_paths)?.join(".claude"),
        };
        let project = match (&options.project_key, ts) {
            (Some(key), _) => key.clone(),
            (None, true) => project_key_ts(cwd, &filesystem, subprocess_environment, environment_mode, environment)?,
            (None, false) => SessionProjectKey::for_directory_with_filesystem(cwd, &filesystem)?,
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
            StoreResume::Latest if ts => latest_candidate_ts(options, &project).await?,
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
        let unique = if ts {
            uuid::Uuid::new_v4().hyphenated().to_string()
        } else {
            uuid::Uuid::new_v4().simple().to_string()
        };
        let path = environment.temp_dir().join(format!("claude-resume-{unique}"));
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
            ts,
            budget: ResumeBudget::default(),
        };
        let auth = AuthSources {
            caller_config_dir: caller_config_dir.as_deref(),
            source_config_dir: &source_config_dir,
            subprocess_environment,
            environment_mode,
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
        .map_err(|_| {
            load_timeout(
                options,
                format!("SessionStore.load() timed out after {{ms}}ms for session {id}"),
                format!("SessionStore.load() for session {id}"),
            )
        })??;
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
    .map_err(|_| {
        load_timeout(
            options,
            "SessionStore.listSessions() timed out after {ms}ms".to_owned(),
            "SessionStore.list_sessions()".to_owned(),
        )
    })??;
    if entry_limit_exceeded(options, sessions.len()) {
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

/// Upstream `O7`/`nnt` continue: `listSessions` sorted newest first (stable), the first session wins with no UUID or
/// sidechain filtering. A first session that is not a UUID, or has no entries, resumes nothing.
async fn latest_candidate_ts(
    options: &SessionStoreOptions,
    project: &SessionProjectKey,
) -> Result<Option<(SessionId, Vec<SessionEntry>)>, SessionError> {
    if !options.store.supports(SessionStoreOperation::List) {
        return Err(SessionError::ContinueRequiresList);
    }
    let mut sessions = timeout(
        options.limits.load_timeout.as_duration(),
        options.store.list_sessions(project),
    )
    .await
    .map_err(|_| {
        load_timeout(
            options,
            "SessionStore.listSessions() timed out after {ms}ms".to_owned(),
            "SessionStore.list_sessions()".to_owned(),
        )
    })??;
    if entry_limit_exceeded(options, sessions.len()) {
        return Err(SessionError::EntryLimit);
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified_at_ms));
    let Some(first) = sessions.into_iter().next() else {
        return Ok(None);
    };
    if !is_uuid(&first.session_id) {
        return Ok(None);
    }
    load_candidate(options, project, first.session_id).await
}

/// The timeout error with the dialect's text; `{ms}` stands for the configured limit in milliseconds.
/// Python words every expiry as `<call> timed out after <ms>ms during resume materialization`.
fn load_timeout(options: &SessionStoreOptions, typescript: String, python: String) -> SessionError {
    let ms = options.limits.load_timeout.as_duration().as_millis().to_string();
    let message = match options.dialect {
        SummaryDialect::TypeScript => typescript,
        SummaryDialect::Python => format!("{python} timed out after {{ms}}ms during resume materialization"),
    };
    SessionError::LoadTimeout(message.replace("{ms}", &ms))
}

fn entry_limit_exceeded(options: &SessionStoreOptions, count: usize) -> bool {
    options
        .limits
        .max_resume_entries
        .is_some_and(|limit| count > limit.get())
}

fn byte_budget(options: &SessionStoreOptions, used: usize) -> usize {
    options
        .limits
        .max_resume_bytes
        .map_or(usize::MAX, |limit| limit.get().saturating_sub(used))
}

fn is_uuid(id: &SessionId) -> bool {
    id.as_str().len() == 36 && uuid::Uuid::parse_str(id.as_str()).is_ok()
}

fn caller_config_dir<E: EnvironmentVariable>(
    options: &SessionStoreOptions,
    environment: &E,
    ts: bool,
) -> Result<Option<PathBuf>, SessionError> {
    let directory = if ts {
        // `options.env?.CLAUDE_CONFIG_DIR ?? process.env.CLAUDE_CONFIG_DIR`: nullish, so an empty value stays.
        options
            .config_directory
            .clone()
            .or_else(|| environment.var_os(OsStr::new("CLAUDE_CONFIG_DIR")).map(PathBuf::from))
    } else {
        // `opt_env.get("CLAUDE_CONFIG_DIR") or os.environ.get("CLAUDE_CONFIG_DIR")`: falsy, so an empty value falls
        // through to the next source and then to the home directory.
        options
            .config_directory
            .clone()
            .filter(|directory| !directory.as_os_str().is_empty())
            .or_else(|| {
                environment
                    .var_os(OsStr::new("CLAUDE_CONFIG_DIR"))
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            })
    };
    match directory {
        Some(directory) if options.require_absolute_config_paths && !directory.is_absolute() => {
            Err(SessionError::UnsafeConfigPath)
        }
        directory => Ok(directory),
    }
}

/// `Wn(directory, env)` reads the project name settings from `options.env` alone when it is given.
fn project_key_ts<F: SessionFileSystem, E: EnvironmentVariable>(
    cwd: &Path,
    filesystem: &F,
    subprocess_environment: &Environment,
    environment_mode: EnvironmentMode,
    environment: &E,
) -> Result<SessionProjectKey, SessionProjectKeyError> {
    let lookup = |name: &str| -> Option<String> {
        let configured = subprocess_environment.get(name).map(|value| value.expose().to_owned());
        if environment_mode == EnvironmentMode::Replace {
            return configured;
        }
        configured.or_else(|| {
            environment
                .var_os(OsStr::new(name))
                .map(|value| value.to_string_lossy().into_owned())
        })
    };
    let config_dir = lookup("CLAUDE_CONFIG_DIR");
    let name_override = lookup("CLAUDE_CODE_PROJECT_DIR_NAME");
    SessionProjectKey::for_directory_ts_with_filesystem(
        cwd,
        filesystem,
        config_dir.as_deref(),
        name_override.as_deref(),
    )
}

/// Both SDKs join a relative home directory as given; `require_absolute` is the opt-in
/// [`SessionStoreOptions::require_absolute_config_paths`] check.
fn home_dir<E: EnvironmentVariable>(environment: &E, require_absolute: bool) -> Result<PathBuf, SessionError> {
    let home = environment
        .var_os(OsStr::new(if cfg!(windows) { "USERPROFILE" } else { "HOME" }))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(SessionError::MissingConfigDirectory)?;
    if require_absolute && !home.is_absolute() {
        return Err(SessionError::UnsafeConfigPath);
    }
    Ok(home)
}

/// Upstream TypeScript points a child resumed from a store at the caller's secure storage on Windows, because the
/// materialized `CLAUDE_CONFIG_DIR` would otherwise move the credential store with it:
/// `env?.CLAUDE_SECURESTORAGE_CONFIG_DIR ?? process.env.CLAUDE_SECURESTORAGE_CONFIG_DIR ?? env?.CLAUDE_CONFIG_DIR ??
/// process.env.CLAUDE_CONFIG_DIR ?? ""`. Python sets nothing.
pub(crate) fn secure_storage_config_dir(
    resumed: bool,
    dialect: Option<SummaryDialect>,
    configured: &Environment,
    inherited: &crate::environment_plan::Inherited,
    windows: bool,
) -> Option<OsString> {
    if !windows || !resumed || dialect != Some(SummaryDialect::TypeScript) {
        return None;
    }
    let configured = |name: &str| configured.get(name).map(|value| OsString::from(value.expose()));
    let inherited = |name: &str| inherited.get(OsStr::new(name)).cloned();
    Some(
        configured("CLAUDE_SECURESTORAGE_CONFIG_DIR")
            .or_else(|| inherited("CLAUDE_SECURESTORAGE_CONFIG_DIR"))
            .or_else(|| configured("CLAUDE_CONFIG_DIR"))
            .or_else(|| inherited("CLAUDE_CONFIG_DIR"))
            .unwrap_or_default(),
    )
}

struct Materialization<'a, F: SessionFileSystem> {
    lease: &'a MaterializedSession<F>,
    store_options: &'a SessionStoreOptions,
    project: &'a SessionProjectKey,
    id: &'a SessionId,
    /// Whether the TypeScript SDK's rules apply (otherwise the Python SDK's).
    ts: bool,
    budget: ResumeBudget,
}

struct AuthSources<'a, E, P> {
    caller_config_dir: Option<&'a Path>,
    source_config_dir: &'a Path,
    subprocess_environment: &'a Environment,
    environment_mode: EnvironmentMode,
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
        create_directories(filesystem, &project_dir, Some(0o700))?;
        let contents = encode_entries(&entries, self.store_options, &mut self.budget, self.ts)?;
        write_private(
            filesystem,
            &project_dir.join(format!("{}.jsonl", self.id.as_str())),
            &contents,
        )?;
        if self.ts {
            copy_auth_files_ts(filesystem, &self.lease.path, auth).await?;
        } else {
            copy_auth_files(filesystem, &self.lease.path, auth).await?;
        }
        if self.store_options.store.supports(SessionStoreOperation::ListSubkeys) {
            self.materialize_subkeys(&project_dir).await?;
        }
        Ok(())
    }

    async fn materialize_subkeys(&mut self, project_dir: &Path) -> Result<(), SessionError> {
        let options = self.store_options;
        let key = SessionKey::new(self.project.clone(), self.id.clone())?;
        let subpaths = timeout(
            options.limits.load_timeout.as_duration(),
            options.store.list_subkeys(&key),
        )
        .await
        .map_err(|_| {
            load_timeout(
                options,
                format!(
                    "SessionStore.listSubkeys() timed out after {{ms}}ms for session {}",
                    self.id
                ),
                format!("SessionStore.list_subkeys() for session {}", self.id),
            )
        })??;
        if options
            .limits
            .max_subkeys
            .is_some_and(|limit| subpaths.len() > limit.get())
        {
            return Err(SessionError::SubpathLimit);
        }
        let session_dir = project_dir.join(self.id.as_str());
        for subpath in subpaths {
            if self.ts && unsafe_subpath_ts(&subpath, &session_dir) {
                tracing::warn!(%subpath, "skipping unsafe subpath from list_subkeys");
                continue;
            }
            let subpath = if self.ts {
                // `Jtt` writes every subpath its check above lets through, `./agent` and `C:agent` included.
                SessionSubpath::listed_by_typescript(subpath)
            } else {
                match SessionSubpath::new(subpath) {
                    Ok(subpath) => subpath,
                    Err(error) => {
                        tracing::warn!(%error, "skipping unsafe subpath from list_subkeys");
                        continue;
                    }
                }
            };
            let subkey = key.clone().with_subpath(subpath.clone());
            let entries = timeout(options.limits.load_timeout.as_duration(), options.store.load(&subkey))
                .await
                .map_err(|_| {
                    load_timeout(
                        options,
                        format!(
                            "SessionStore.load() timed out after {{ms}}ms for session {} subpath {subpath}",
                            self.id
                        ),
                        format!("SessionStore.load() for session {} subpath {subpath}", self.id),
                    )
                })??;
            let Some(entries) = entries.filter(|entries| !entries.is_empty()) else {
                continue;
            };
            if self.ts {
                self.write_subpath_ts(&session_dir, &subpath, entries)?;
            } else {
                self.write_subpath(&session_dir, &subpath, entries)?;
            }
        }
        Ok(())
    }

    /// Upstream TypeScript `Jtt` body: transcript entries to `<subpath>.jsonl`, the last metadata entry (without its
    /// `type`) to `<subpath>.meta.json` with no trailing newline, in directories created with the default mode.
    fn write_subpath_ts(
        &mut self,
        session_dir: &Path,
        subpath: &SessionSubpath,
        entries: Vec<SessionEntry>,
    ) -> Result<(), SessionError> {
        let options = self.store_options;
        let filesystem = &self.lease.filesystem;
        let is_metadata = |entry: &SessionEntry| entry.kind() == "agent_metadata";
        let metadata = entries.iter().rev().find(|entry| is_metadata(entry));
        let transcript: Vec<_> = entries.iter().filter(|entry| !is_metadata(entry)).cloned().collect();
        if !transcript.is_empty() {
            let target = lexical_resolve(&session_dir.join(format!("{subpath}.jsonl")));
            create_directories(filesystem, target.parent().unwrap_or(session_dir), None)?;
            let contents = encode_entries(&transcript, options, &mut self.budget, true)?;
            write_private(filesystem, &target, &contents)?;
        }
        if let Some(metadata) = metadata {
            let target = lexical_resolve(&session_dir.join(format!("{subpath}.meta.json")));
            create_directories(filesystem, target.parent().unwrap_or(session_dir), None)?;
            let mut rest = metadata.as_object().clone();
            rest.shift_remove("type");
            let mut writer = ResumeWriter::new(byte_budget(options, self.budget.bytes));
            writer.encode_js(&rest)?;
            let contents = writer.bytes;
            self.budget.entries = self.budget.entries.saturating_add(1);
            self.budget.bytes = self.budget.bytes.saturating_add(contents.len());
            if entry_limit_exceeded(options, self.budget.entries) {
                return Err(SessionError::EntryLimit);
            }
            write_private(filesystem, &target, &contents)?;
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
        create_directories(filesystem, directory, Some(0o700))?;
        if !transcript.is_empty() {
            let contents = encode_entries(&transcript, options, &mut self.budget, false)?;
            write_private(filesystem, &directory.join(format!("{name}.jsonl")), &contents)?;
        }
        if let Some(metadata) = metadata {
            let mut object = metadata.as_object().clone();
            object.remove("type");
            let mut writer = ResumeWriter::new(byte_budget(options, self.budget.bytes));
            writer.encode(&object)?;
            let contents = writer.bytes;
            self.budget.entries = self.budget.entries.saturating_add(1);
            self.budget.bytes = self.budget.bytes.saturating_add(contents.len());
            if entry_limit_exceeded(options, self.budget.entries) {
                return Err(SessionError::EntryLimit);
            }
            write_private(filesystem, &directory.join(format!("{name}.meta.json")), &contents)?;
        }
        Ok(())
    }
}

/// Upstream TypeScript `Jtt` safety check: a subpath must be non-empty, relative, free of `..` segments and resolve
/// inside the session directory.
fn unsafe_subpath_ts(subpath: &str, session_dir: &Path) -> bool {
    if subpath.is_empty()
        || Path::new(subpath).is_absolute()
        || subpath.starts_with(['/', '\\'])
        || subpath.split(['/', '\\']).any(|segment| segment == "..")
    {
        return true;
    }
    let resolved = lexical_resolve(&session_dir.join(format!("{subpath}.jsonl")));
    let mut prefix = session_dir.as_os_str().to_os_string();
    prefix.push(std::path::MAIN_SEPARATOR_STR);
    !resolved
        .as_os_str()
        .to_string_lossy()
        .starts_with(&*prefix.to_string_lossy())
}

fn create_directories<F: SessionFileSystem>(
    filesystem: &F,
    path: &Path,
    mode: Option<u32>,
) -> Result<(), SessionError> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
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
        environment_mode,
        environment,
        process,
        keychain,
    } = auth;
    let mut credentials = read_if_present(filesystem, &source_config_dir.join(".credentials.json"));
    let has_auth_variable = |name: &str| auth_variable_set(subprocess_environment, environment_mode, environment, name);
    if keychain
        && caller_config_dir.is_none()
        && !has_auth_variable("ANTHROPIC_API_KEY")
        && !has_auth_variable("CLAUDE_CODE_OAUTH_TOKEN")
        && let Some(keychain) = read_keychain_credentials(environment, process, KEYCHAIN_SERVICE_NAME, false).await
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
        None => home_dir(environment, false)?.join(".claude.json"),
    };
    copy_if_present(
        filesystem,
        &claude_json,
        &config_dir.join(".claude.json"),
        CopyMode::Private,
    );
    for name in ["settings.json", "cowork_settings.json"] {
        copy_if_present(
            filesystem,
            &source_config_dir.join(name),
            &config_dir.join(name),
            CopyMode::Transformed(strip_settings_for_resume),
        );
    }
    Ok(())
}

/// The variables that configure authentication count when the subprocess overrides or this process sets them.
fn auth_variable_set<E: EnvironmentVariable>(
    subprocess_environment: &Environment,
    mode: EnvironmentMode,
    environment: &E,
    name: &str,
) -> bool {
    subprocess_environment
        .get(name)
        .is_some_and(|value| !value.expose().is_empty())
        || (mode != EnvironmentMode::Replace
            && environment
                .var_os(OsStr::new(name))
                .is_some_and(|value| !value.is_empty()))
}

/// Upstream TypeScript `p7(d7)`: the Keychain service the CLI stores credentials under. It reads this process's
/// environment, never `options.env`.
fn keychain_service_ts<E: EnvironmentVariable>(environment: &E) -> Result<String, SessionError> {
    let text = |name: &str| {
        environment
            .var_os(OsStr::new(name))
            .map(|value| value.to_string_lossy().into_owned())
    };
    let suffix = match text("CLAUDE_CODE_CUSTOM_OAUTH_URL").filter(|url| !url.is_empty()) {
        Some(url) => {
            let endpoint = url.strip_suffix('/').unwrap_or(&url);
            if !APPROVED_OAUTH_ENDPOINTS_TS.contains(&endpoint) {
                return Err(SessionError::UnapprovedOauthEndpoint);
            }
            "-custom-oauth"
        }
        None => "",
    };
    // A set secure storage directory decides alone, even when empty; an empty directory keeps the default service.
    let source = match text("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
        Some(directory) => directory,
        None => text("CLAUDE_CONFIG_DIR").unwrap_or_default(),
    };
    let hash = if source.is_empty() {
        String::new()
    } else {
        let digest = Sha256::digest(source.nfc().collect::<String>().as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("-{}", &hex[..8])
    };
    Ok(format!("Claude Code{suffix}-credentials{hash}"))
}

/// Reads a regular file, reporting every failure including a missing file.
fn read_regular<F: SessionFileSystem>(filesystem: &F, source: &Path) -> io::Result<Vec<u8>> {
    if !filesystem.metadata(source)?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut contents = Vec::new();
    filesystem
        .open(OpenOptions::new().read(true), source)?
        .read_to_end(&mut contents)?;
    Ok(contents)
}

/// Upstream TypeScript `A7` credential and configuration copies. A credentials file that cannot be read for any reason
/// but its absence fails the resume; the other files are skipped with a warning, as upstream does.
async fn copy_auth_files_ts<F, E, P>(
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
        environment_mode,
        environment,
        process,
        keychain,
    } = auth;
    let mut credentials = match read_regular(filesystem, &source_config_dir.join(".credentials.json")) {
        Ok(contents) => Some(String::from_utf8_lossy(&contents).into_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(SessionError::Io(error)),
    };
    let configured_directory = caller_config_dir.filter(|directory| !directory.as_os_str().is_empty());
    if keychain
        && configured_directory.is_none()
        && !auth_variable_set(
            subprocess_environment,
            environment_mode,
            environment,
            "ANTHROPIC_API_KEY",
        )
        && !auth_variable_set(
            subprocess_environment,
            environment_mode,
            environment,
            "CLAUDE_CODE_OAUTH_TOKEN",
        )
    {
        let service = keychain_service_ts(environment)?;
        if let Some(stored) = read_keychain_credentials(environment, process, &service, true).await {
            credentials = Some(String::from_utf8_lossy(&stored).into_owned());
        }
    }
    if let Some(credentials) = credentials.filter(|credentials| !credentials.is_empty()) {
        write_private(
            filesystem,
            &config_dir.join(".credentials.json"),
            redact_refresh_token_ts(credentials).as_bytes(),
        )?;
    }
    let base = match configured_directory {
        Some(directory) => directory.to_path_buf(),
        None => home_dir(environment, false)?,
    };
    copy_if_present(
        filesystem,
        &base.join(".claude.json"),
        &config_dir.join(".claude.json"),
        CopyMode::SourceMode,
    );
    copy_if_present(
        filesystem,
        &source_config_dir.join(".config.json"),
        &config_dir.join(".config.json"),
        CopyMode::SourceMode,
    );
    for suffix in OAUTH_FILE_SUFFIXES_TS {
        let name = format!(".claude{suffix}.json");
        copy_if_present(
            filesystem,
            &base.join(&name),
            &config_dir.join(&name),
            CopyMode::SourceMode,
        );
    }
    for name in ["settings.json", "cowork_settings.json"] {
        copy_if_present(
            filesystem,
            &source_config_dir.join(name),
            &config_dir.join(name),
            CopyMode::Transformed(strip_settings_for_resume_ts),
        );
    }
    Ok(())
}

/// Upstream TypeScript `Ytt`: the refresh token is dropped when truthy and the document re-serialized with
/// `JSON.stringify`; anything that does not parse is written as read.
fn redact_refresh_token_ts(credentials: String) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&credentials) else {
        return credentials;
    };
    let serde_json::Value::Object(mut root) = js_ordered(parsed) else {
        return credentials;
    };
    let truthy = match root
        .get("claudeAiOauth")
        .and_then(serde_json::Value::as_object)
        .and_then(|oauth| oauth.get("refreshToken"))
    {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(false)) => false,
        Some(serde_json::Value::String(text)) => !text.is_empty(),
        Some(serde_json::Value::Number(number)) => number.as_f64().is_none_or(|value| value != 0.0),
        Some(_) => true,
    };
    if !truthy {
        return credentials;
    }
    if let Some(serde_json::Value::Object(oauth)) = root.get_mut("claudeAiOauth") {
        oauth.shift_remove("refreshToken");
    }
    js_stringify(&serde_json::Value::Object(root))
}

/// Upstream TypeScript `_7`: drops the plugin and marketplace settings and `env.CLAUDE_CONFIG_DIR`, re-serialized
/// with `JSON.stringify`. A file that does not parse to an object, or has nothing to drop, is copied as read.
fn strip_settings_for_resume_ts(contents: Vec<u8>) -> Vec<u8> {
    let text = String::from_utf8_lossy(&contents);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
        return contents;
    };
    let serde_json::Value::Object(mut root) = js_ordered(parsed) else {
        return contents;
    };
    let mut stripped = false;
    for key in RESUME_SETTINGS_STRIPPED_KEYS_TS {
        stripped |= root.shift_remove(key).is_some();
    }
    if let Some(serde_json::Value::Object(env)) = root.get_mut("env") {
        stripped |= env.shift_remove("CLAUDE_CONFIG_DIR").is_some();
    }
    if !stripped {
        return contents;
    }
    js_stringify(&serde_json::Value::Object(root)).into_bytes()
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

/// How a configuration file is copied into the resume directory.
#[derive(Clone, Copy)]
enum CopyMode {
    /// Private (`0600`) with the contents unchanged.
    Private,
    /// Private (`0600`) with the contents rewritten.
    Transformed(fn(Vec<u8>) -> Vec<u8>),
    /// Byte for byte, with the source file's permission bits, as `fs.copyFile` does.
    SourceMode,
}

fn copy_if_present<F: SessionFileSystem>(filesystem: &F, source: &Path, destination: &Path, copy: CopyMode) {
    let Some(contents) = read_if_present(filesystem, source) else {
        return;
    };
    let result = match copy {
        CopyMode::Private => write_private_file(filesystem, destination, &contents),
        CopyMode::Transformed(transform) => write_private_file(filesystem, destination, &transform(contents)),
        CopyMode::SourceMode => write_with_source_mode(filesystem, source, destination, &contents),
    };
    if let Err(error) = result {
        let _ = filesystem.remove_file(destination);
        tracing::warn!(path = %source.display(), %error, "resume: skipping configuration file that could not be copied");
    }
}

fn write_with_source_mode<F: SessionFileSystem>(
    filesystem: &F,
    source: &Path,
    destination: &Path,
    contents: &[u8],
) -> io::Result<()> {
    let permissions = filesystem.metadata(source)?.permissions();
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = filesystem.open(&options, destination)?;
    file.write_all(contents)?;
    file.set_permissions(permissions)
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
async fn read_keychain_credentials<E, P>(environment: &E, process: &P, service: &str, ts: bool) -> Option<Vec<u8>>
where
    E: EnvironmentVariable,
    P: SpawnProcess<Error = io::Error>,
{
    let user = match environment.var_os(OsStr::new("USER")).filter(|value| !value.is_empty()) {
        Some(user) => user,
        None => login_name(process)
            .await
            .unwrap_or_else(|| OsString::from(KEYCHAIN_FALLBACK_ACCOUNT)),
    };
    // TypeScript `f7` only accepts `[a-zA-Z0-9._-]+`.
    let account = if ts
        && !user.to_str().is_some_and(|name| {
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        }) {
        OsString::from(KEYCHAIN_FALLBACK_ACCOUNT)
    } else {
        user
    };
    let mut command = tokio::process::Command::new("security");
    command
        .arg("find-generic-password")
        .arg("-a")
        .arg(account)
        .args(["-w", "-s", service])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = process.spawn(&mut command).ok()?;
    let output = timeout(KEYCHAIN_TIMEOUT, child.wait_with_output()).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    // Node decodes the output as UTF-8 with replacement; Python's text mode fails on invalid bytes.
    let text = if ts {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        String::from_utf8(output.stdout).ok()?
    };
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.as_bytes().to_vec())
}

/// `os.userInfo().username` / `getpass.getuser()` when `USER` is unset: the passwd name of the current user.
async fn login_name<P: SpawnProcess<Error = io::Error>>(process: &P) -> Option<OsString> {
    let mut command = tokio::process::Command::new("id");
    command
        .arg("-un")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = process.spawn(&mut command).ok()?;
    let output = timeout(KEYCHAIN_TIMEOUT, child.wait_with_output()).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8(output.stdout).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| OsString::from(name))
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
    ts: bool,
) -> Result<Vec<u8>, SessionError> {
    budget.entries = budget.entries.saturating_add(entries.len());
    if entry_limit_exceeded(options, budget.entries) {
        return Err(SessionError::EntryLimit);
    }
    let mut writer = ResumeWriter::new(byte_budget(options, budget.bytes));
    for entry in entries {
        if ts {
            writer.encode_js(entry.as_object())?;
        } else {
            writer.encode(entry.as_object())?;
        }
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

    /// One `JSON.stringify` document.
    fn encode_js(&mut self, object: &JsonObject) -> Result<(), SessionError> {
        let text = js_stringify(&serde_json::Value::Object(object.clone()));
        self.write_all(text.as_bytes()).map_err(|_| SessionError::ByteLimit)
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
