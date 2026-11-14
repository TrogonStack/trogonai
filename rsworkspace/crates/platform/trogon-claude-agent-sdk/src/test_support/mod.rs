use crate::adapters::{CurrentDirectory, EnvironmentVariable, TemporaryDirectory};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

#[derive(Clone, Debug)]
pub struct FixedEnvironment {
    pub directory: PathBuf,
    pub variables: BTreeMap<OsString, OsString>,
}

impl EnvironmentVariable for FixedEnvironment {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        self.variables.get(key).cloned()
    }

    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        self.variables.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    fn effective_user_home_directory(&self) -> Option<OsString> {
        None
    }

    fn real_user_home_directory(&self) -> Option<OsString> {
        None
    }
}

/// Wraps an environment and answers the two passwd lookups with fixed entries, so a test decides what the
/// passwd database holds for the effective and the real user.
#[derive(Clone, Debug)]
pub struct WithPasswd<E> {
    pub environment: E,
    pub effective: Option<OsString>,
    pub real: Option<OsString>,
}

impl<E: EnvironmentVariable> EnvironmentVariable for WithPasswd<E> {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        self.environment.var_os(key)
    }

    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        self.environment.vars_os()
    }

    fn effective_user_home_directory(&self) -> Option<OsString> {
        self.effective.clone()
    }

    fn real_user_home_directory(&self) -> Option<OsString> {
        self.real.clone()
    }
}

impl CurrentDirectory for FixedEnvironment {
    type Error = std::io::Error;
    fn current_dir(&self) -> Result<PathBuf, Self::Error> {
        Ok(self.directory.clone())
    }
}

/// Mirrors `std::env::temp_dir` on Unix: `TMPDIR` when set, else the fixed working directory.
impl TemporaryDirectory for FixedEnvironment {
    fn temp_dir(&self) -> PathBuf {
        self.variables
            .get(OsStr::new("TMPDIR"))
            .map_or_else(|| self.directory.clone(), PathBuf::from)
    }
}

/// The Windows path rules from the upstream SDKs, runnable on any host so a Linux build can test them.
pub mod windows {
    use crate::{SessionKey, SessionKeyError, SummaryDialect};
    use std::path::Path;

    /// `SessionKey::from_transcript_path` with the Windows rules, a dialect, a fixed working directory and text paths.
    pub fn transcript_key(
        file_path: &str,
        projects_dir: &str,
        cwd: &str,
        dialect: SummaryDialect,
    ) -> Result<SessionKey, SessionKeyError> {
        SessionKey::from_transcript_path_on(
            Path::new(file_path),
            Path::new(projects_dir),
            Path::new(cwd),
            true,
            dialect,
        )
    }

    /// The TypeScript resume guard: true when the subpath is skipped as unsafe.
    pub fn typescript_subpath_is_unsafe(subpath: &str, session_dir: &str) -> bool {
        crate::session_resume::unsafe_subpath_ts(subpath, Path::new(session_dir), true)
    }

    /// The Python resume guard: true when the subpath is accepted.
    pub fn python_subpath_is_safe(subpath: &str) -> bool {
        crate::session_subpath::python_accepts(subpath)
    }

    /// `SessionSubpath::checked`, Rust's own rule, for a platform.
    pub fn checked_subpath_is_safe(subpath: &str, windows: bool) -> bool {
        crate::session_subpath::is_safe(subpath, windows)
    }

    /// The name a directory listing compares on: lowercase on Windows, as both SDKs fold it there.
    pub fn fold_directory_name(name: &str, windows: bool) -> String {
        crate::session_project_key::fold_directory_name(name, windows)
    }
}
