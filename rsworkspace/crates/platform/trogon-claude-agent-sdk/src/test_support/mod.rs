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
