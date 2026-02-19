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
        Self {
            dirs: HashMap::new(),
        }
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
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn default_returns_none_for_all() {
        let dirs = FixedDirs::default();
        assert!(dirs.home_dir().is_none());
        assert!(dirs.config_dir().is_none());
        assert!(dirs.cache_dir().is_none());
        assert!(dirs.data_dir().is_none());
        assert!(dirs.data_local_dir().is_none());
        assert!(dirs.state_dir().is_none());
    }

    #[test]
    fn set_and_get() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Home, "/home/test");
        dirs.set(DirKind::Config, "/etc/myapp");

        assert_eq!(dirs.home_dir(), Some(PathBuf::from("/home/test")));
        assert_eq!(dirs.config_dir(), Some(PathBuf::from("/etc/myapp")));
        assert!(dirs.cache_dir().is_none());
    }

    #[test]
    fn clear_removes_all() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Home, "/home/test");
        dirs.set(DirKind::Cache, "/tmp/cache");
        dirs.clear();

        assert!(dirs.home_dir().is_none());
        assert!(dirs.cache_dir().is_none());
    }

    #[test]
    fn set_overwrites() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Data, "/old");
        dirs.set(DirKind::Data, "/new");

        assert_eq!(dirs.data_dir(), Some(PathBuf::from("/new")));
    }

    #[test]
    fn generic_function_with_fixed_dirs() {
        fn get_state<D: StateDir>(dirs: &D, app: &str) -> Option<PathBuf> {
            dirs.state_dir().map(|d| d.join(app))
        }

        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::State, "/var/state");

        assert_eq!(
            get_state(&dirs, "myapp"),
            Some(PathBuf::from("/var/state/myapp"))
        );
    }

    #[test]
    fn chained_set_calls() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Home, "/home")
            .set(DirKind::Config, "/config")
            .set(DirKind::Cache, "/cache");

        assert_eq!(dirs.home_dir(), Some(PathBuf::from("/home")));
        assert_eq!(dirs.config_dir(), Some(PathBuf::from("/config")));
        assert_eq!(dirs.cache_dir(), Some(PathBuf::from("/cache")));
    }
}
