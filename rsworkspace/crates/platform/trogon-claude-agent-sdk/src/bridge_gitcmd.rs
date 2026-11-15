//! The gated Git subprocess of the upstream bridge, used to read references the files reader cannot open (the table
//! format).
//!
//! Upstream only runs Git when nothing blocks the host, when a session working directory exists, and when that
//! directory belongs to the same repository as the one being asked about. Each of those global inputs is an explicit
//! field of [`GitProbeContext`], and the default is what upstream sees in a standalone process: nothing blocked, the
//! current directory as the session working directory and the real environment.
//!
//! The upstream runner also wraps the child in a sandbox, a memory cgroup and a launcher probe. Those wrappers only
//! restrict the child and never change what Git prints, so they are not reproduced; the five second limit, the
//! killing of a child that outlives it and the sanitised environment are.

use std::collections::BTreeMap;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crate::adapters::{EnvironmentVariable, SystemEnvironment};
use crate::bridge_gitdir::{common_dir, find_git_dir};
use crate::bridge_paths::PathGuard;

mod constants;

use constants::{CONFIG_PARAMETERS, GIT_FLAGS, RUN_LIMIT, SCRUBBED};

/// What the upstream process-wide state says to the Git runner.
#[derive(Clone, Debug)]
pub struct GitProbeContext {
    /// The host is blocked from running Git at all.
    pub blocked: bool,
    /// The working directory of the session; Git only runs for repositories that belong to it.
    pub session_cwd: Option<PathBuf>,
    /// The environment the child starts from, before sanitising.
    pub environment: Vec<(OsString, OsString)>,
    /// The Git executable.
    pub git_program: PathBuf,
    /// How long a call may take before the child is killed.
    pub timeout: Duration,
    /// The path checks and the host they ask.
    pub paths: Arc<PathGuard>,
}

impl GitProbeContext {
    /// What a standalone process sees: nothing blocked, the current directory as the session directory (which is what
    /// session creation falls back to when no directory is given), the real environment.
    pub fn from_process() -> Self {
        Self {
            blocked: false,
            session_cwd: std::env::current_dir().ok(),
            environment: SystemEnvironment.vars_os(),
            git_program: PathBuf::from("git"),
            timeout: RUN_LIMIT,
            paths: PathGuard::host(),
        }
    }

    /// The same context with other path checks.
    #[must_use]
    pub fn with_paths(mut self, paths: Arc<PathGuard>) -> Self {
        self.paths = paths;
        self
    }

    /// The same context for a session that works in `directory`.
    #[must_use]
    pub fn with_session_cwd(mut self, directory: impl Into<PathBuf>) -> Self {
        self.session_cwd = Some(directory.into());
        self
    }
}

impl Default for GitProbeContext {
    fn default() -> Self {
        Self::from_process()
    }
}

/// What a finished Git call printed.
pub(crate) struct GitOutput {
    pub(crate) success: bool,
    pub(crate) stdout: String,
}

#[derive(Clone, Copy)]
struct FileId {
    directory: bool,
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> FileId {
    FileId {
        directory: meta.is_dir(),
        device: meta.dev(),
        inode: meta.ino(),
    }
}

/// Without stable file identities the comparison falls back to the path equality branch, as it does upstream when
/// the platform reports a zero device.
#[cfg(not(unix))]
fn file_id(meta: &std::fs::Metadata) -> FileId {
    FileId {
        directory: meta.is_dir(),
        device: 0,
        inode: 0,
    }
}

async fn identity(path: &Path) -> Option<FileId> {
    tokio::fs::metadata(path).await.ok().map(|meta| file_id(&meta))
}

fn unusable_inode(inode: u64) -> bool {
    inode == 0 || inode == u64::MAX
}

async fn real(path: &Path) -> Option<PathBuf> {
    tokio::fs::canonicalize(path).await.ok()
}

impl GitProbeContext {
    /// Runs Git for the repository directory `repository`, only when the trust gating allows it.
    pub(crate) async fn run(&self, repository: &Path, args: &[&str]) -> Option<GitOutput> {
        if self.blocked {
            return None;
        }
        let common = common_dir(&self.paths, repository).unwrap_or_else(|| repository.to_path_buf());
        let session_git = find_git_dir(&self.paths, self.session_cwd.as_deref()?)?;
        let session_common = common_dir(&self.paths, &session_git).unwrap_or(session_git);
        let (git_dir, common_real, session_real) = tokio::join!(real(repository), real(&common), real(&session_common));
        let (git_dir, common_real, session_real) = (git_dir?, common_real?, session_real?);
        if git_dir != common_real && git_dir.parent() != Some(common_real.join("worktrees").as_path()) {
            return None;
        }
        let (target, session, parent) = tokio::join!(identity(&common_real), identity(&session_real), async {
            match common_real.parent() {
                Some(parent) => identity(parent).await,
                None => None,
            }
        });
        let target = target.filter(|id| id.directory)?;
        let distinct = parent.is_some_and(|parent| {
            target.device != 0
                && !unusable_inode(target.inode)
                && !(target.device == parent.device && target.inode == parent.inode)
        });
        let allowed = if distinct {
            session.is_some_and(|id| id.device == target.device && id.inode == target.inode)
        } else {
            common_real == session_real
        };
        if !allowed {
            return None;
        }
        Some(self.spawn(&git_dir, &common_real, args).await)
    }

    async fn spawn(&self, git_dir: &Path, common: &Path, args: &[&str]) -> GitOutput {
        let mut command = tokio::process::Command::new(&self.git_program);
        command
            .args(GIT_FLAGS)
            .args(args)
            .current_dir(git_dir)
            .env_clear()
            .envs(sanitised_environment(&self.environment, git_dir, common))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        match tokio::time::timeout(self.timeout, command.output()).await {
            Ok(Ok(output)) => GitOutput {
                success: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            },
            Ok(Err(_)) | Err(_) => GitOutput {
                success: false,
                stdout: String::new(),
            },
        }
    }
}

fn config_slot(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let Some(rest) = upper.strip_prefix("GIT_CONFIG_") else {
        return false;
    };
    let Some(digits) = rest.strip_prefix("KEY_").or_else(|| rest.strip_prefix("VALUE_")) else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Keeps only the absolute entries of a search path, the way the upstream sanitiser does on POSIX.
fn clean_search_path(value: &str) -> String {
    let kept: Vec<&str> = value
        .split(':')
        .filter(|entry| !entry.is_empty() && entry.starts_with('/'))
        .collect();
    if kept.is_empty() {
        "/dev/null".to_owned()
    } else {
        kept.join(":")
    }
}

/// The environment of a Git call: the base environment minus every variable that could redirect Git, plus the fixed
/// settings that switch off prompts, hooks, aliases and remote transports.
pub(crate) fn sanitised_environment(
    base: &[(OsString, OsString)],
    git_dir: &Path,
    common: &Path,
) -> Vec<(OsString, OsString)> {
    let mut fixed: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in [
        ("GIT_ALLOW_PROTOCOL", "/"),
        ("GIT_NO_LAZY_FETCH", "1"),
        ("GIT_NO_REPLACE_OBJECTS", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_LFS_SKIP_SMUDGE", "1"),
        ("LC_ALL", "C"),
        ("LANGUAGE", "C"),
        ("GIT_SSH_COMMAND", "false"),
        ("GIT_IMPLICIT_WORK_TREE", "0"),
    ] {
        fixed.insert(key.to_owned(), value.to_owned());
    }
    fixed.insert("GIT_CONFIG_PARAMETERS".to_owned(), CONFIG_PARAMETERS.join(" "));
    fixed.insert("GIT_DIR".to_owned(), git_dir.to_string_lossy().into_owned());
    fixed.insert("GIT_COMMON_DIR".to_owned(), common.to_string_lossy().into_owned());
    let fixed_upper: Vec<String> = fixed.keys().map(|key| key.to_ascii_uppercase()).collect();
    let mut result: Vec<(OsString, OsString)> = Vec::new();
    for (key, value) in base {
        let Some(name) = key.to_str() else {
            result.push((key.clone(), value.clone()));
            continue;
        };
        let upper = name.to_ascii_uppercase();
        let dropped = SCRUBBED.contains(&upper.as_str())
            || config_slot(name)
            || (fixed_upper.contains(&upper) && !fixed.contains_key(name));
        if dropped {
            continue;
        }
        if name == "PATH" {
            result.push((key.clone(), clean_search_path(&value.to_string_lossy()).into()));
        } else {
            result.push((key.clone(), value.clone()));
        }
    }
    result.retain(|(key, _)| key.to_str().is_none_or(|name| !fixed.contains_key(name)));
    result.extend(fixed.into_iter().map(|(key, value)| (key.into(), value.into())));
    result
}

#[cfg(test)]
#[path = "bridge_gitcmd_tests.rs"]
mod tests;
