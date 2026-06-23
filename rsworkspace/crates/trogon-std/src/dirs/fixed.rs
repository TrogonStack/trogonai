#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "test-support"))]
use std::path::PathBuf;

#[cfg(any(test, feature = "test-support"))]
use super::{CacheDir, ConfigDir, DataDir, DataLocalDir, HomeDir, StateDir};

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirKind {
    Home,
    Config,
    Cache,
    Data,
    DataLocal,
    State,
}

#[cfg(any(test, feature = "test-support"))]
pub struct FixedDirs {
    dirs: HashMap<DirKind, PathBuf>,
}

#[cfg(any(test, feature = "test-support"))]
impl FixedDirs {
    pub fn new() -> Self {
        Self { dirs: HashMap::new() }
    }

    pub fn set(&mut self, kind: DirKind, path: impl Into<PathBuf>) -> &mut Self {
        self.dirs.insert(kind, path.into());
        self
    }

    pub fn clear(&mut self) -> &mut Self {
        self.dirs.clear();
        self
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for FixedDirs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl HomeDir for FixedDirs {
    fn home_dir(&self) -> Option<PathBuf> {
        self.dirs.get(&DirKind::Home).cloned()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ConfigDir for FixedDirs {
    fn config_dir(&self) -> Option<PathBuf> {
        self.dirs.get(&DirKind::Config).cloned()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl CacheDir for FixedDirs {
    fn cache_dir(&self) -> Option<PathBuf> {
        self.dirs.get(&DirKind::Cache).cloned()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl DataDir for FixedDirs {
    fn data_dir(&self) -> Option<PathBuf> {
        self.dirs.get(&DirKind::Data).cloned()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl DataLocalDir for FixedDirs {
    fn data_local_dir(&self) -> Option<PathBuf> {
        self.dirs.get(&DirKind::DataLocal).cloned()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl StateDir for FixedDirs {
    fn state_dir(&self) -> Option<PathBuf> {
        self.dirs.get(&DirKind::State).cloned()
    }
}

#[cfg(test)]
mod tests;
