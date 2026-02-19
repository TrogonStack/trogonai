use std::path::PathBuf;

use super::{CacheDir, ConfigDir, DataDir, DataLocalDir, HomeDir, StateDir};

pub struct SystemDirs;

impl HomeDir for SystemDirs {
    #[inline]
    fn home_dir(&self) -> Option<PathBuf> {
        home_dir_impl()
    }
}

impl ConfigDir for SystemDirs {
    #[inline]
    fn config_dir(&self) -> Option<PathBuf> {
        config_dir_impl()
    }
}

impl CacheDir for SystemDirs {
    #[inline]
    fn cache_dir(&self) -> Option<PathBuf> {
        cache_dir_impl()
    }
}

impl DataDir for SystemDirs {
    #[inline]
    fn data_dir(&self) -> Option<PathBuf> {
        data_dir_impl()
    }
}

impl DataLocalDir for SystemDirs {
    #[inline]
    fn data_local_dir(&self) -> Option<PathBuf> {
        data_local_dir_impl()
    }
}

impl StateDir for SystemDirs {
    #[inline]
    fn state_dir(&self) -> Option<PathBuf> {
        state_dir_impl()
    }
}

fn non_empty_var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn home_dir_impl() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        non_empty_var("USERPROFILE")
    } else {
        non_empty_var("HOME")
    }
}

fn config_dir_impl() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        home_dir_impl().map(|h| h.join("Library").join("Application Support"))
    } else if cfg!(target_os = "windows") {
        non_empty_var("APPDATA")
    } else {
        non_empty_var("XDG_CONFIG_HOME").or_else(|| home_dir_impl().map(|h| h.join(".config")))
    }
}

fn cache_dir_impl() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        home_dir_impl().map(|h| h.join("Library").join("Caches"))
    } else if cfg!(target_os = "windows") {
        non_empty_var("LOCALAPPDATA")
    } else {
        non_empty_var("XDG_CACHE_HOME").or_else(|| home_dir_impl().map(|h| h.join(".cache")))
    }
}

fn data_dir_impl() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        home_dir_impl().map(|h| h.join("Library").join("Application Support"))
    } else if cfg!(target_os = "windows") {
        non_empty_var("APPDATA")
    } else {
        non_empty_var("XDG_DATA_HOME")
            .or_else(|| home_dir_impl().map(|h| h.join(".local").join("share")))
    }
}

fn data_local_dir_impl() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        home_dir_impl().map(|h| h.join("Library").join("Application Support"))
    } else if cfg!(target_os = "windows") {
        non_empty_var("LOCALAPPDATA")
    } else {
        non_empty_var("XDG_DATA_HOME")
            .or_else(|| home_dir_impl().map(|h| h.join(".local").join("share")))
    }
}

// macOS and Windows have no native state directory concept — returning None
// avoids silently aliasing ~/Library/Application Support or LOCALAPPDATA.
fn state_dir_impl() -> Option<PathBuf> {
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        None
    } else {
        non_empty_var("XDG_STATE_HOME")
            .or_else(|| home_dir_impl().map(|h| h.join(".local").join("state")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_returns_some() {
        let dirs = SystemDirs;
        assert!(dirs.home_dir().is_some());
    }

    #[test]
    fn config_dir_returns_some() {
        let dirs = SystemDirs;
        assert!(dirs.config_dir().is_some());
    }

    #[test]
    fn cache_dir_returns_some() {
        let dirs = SystemDirs;
        assert!(dirs.cache_dir().is_some());
    }

    #[test]
    fn data_dir_returns_some() {
        let dirs = SystemDirs;
        assert!(dirs.data_dir().is_some());
    }

    #[test]
    fn data_local_dir_returns_some() {
        let dirs = SystemDirs;
        assert!(dirs.data_local_dir().is_some());
    }

    #[test]
    fn state_dir_depends_on_platform() {
        let dirs = SystemDirs;
        if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
            assert!(dirs.state_dir().is_none());
        } else {
            assert!(dirs.state_dir().is_some());
        }
    }

    #[test]
    fn generic_function_with_system_dirs() {
        fn get_config_path<D: ConfigDir>(dirs: &D, app: &str) -> Option<PathBuf> {
            dirs.config_dir().map(|d| d.join(app))
        }

        let path = get_config_path(&SystemDirs, "myapp");
        assert!(path.is_some());
        assert!(path.unwrap().ends_with("myapp"));
    }
}
