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
