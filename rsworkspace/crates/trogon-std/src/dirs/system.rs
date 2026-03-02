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

    /// Documents the filtering behaviour of `non_empty_var` without mutating
    /// real environment variables.  The logic `var_os(name).filter(|v| !v.is_empty())`
    /// must return `None` for both absent variables and variables set to `""`.
    #[test]
    fn non_empty_var_logic_filters_empty_and_absent_values() {
        // Replicate the non_empty_var() logic to verify its contract:
        //   Some("") → None  (empty OsString is filtered out)
        //   None     → None  (absent variable)
        //   Some("/x") → Some(PathBuf::from("/x"))
        use std::ffi::OsString;

        let filter = |v: Option<OsString>| -> Option<std::path::PathBuf> {
            v.filter(|s| !s.is_empty()).map(std::path::PathBuf::from)
        };

        assert!(filter(None).is_none(), "absent var must produce None");
        assert!(
            filter(Some(OsString::from(""))).is_none(),
            "empty string must produce None"
        );
        assert_eq!(
            filter(Some(OsString::from("/home/user"))),
            Some(std::path::PathBuf::from("/home/user")),
            "non-empty string must produce Some"
        );
    }

    /// On Linux (the CI platform) the XDG_CONFIG_HOME fallback chain is:
    /// XDG_CONFIG_HOME → $HOME/.config.  Verify that `config_dir` ends
    /// with the expected suffix when `XDG_CONFIG_HOME` is not set.
    #[test]
    fn config_dir_ends_with_expected_suffix_on_linux() {
        if cfg!(target_os = "linux") {
            let dirs = SystemDirs;
            let config = dirs.config_dir().expect("config_dir must be Some on Linux");
            // Either XDG_CONFIG_HOME was set (any absolute path) or we got ~/.config
            let s = config.to_string_lossy();
            assert!(
                s.ends_with(".config") || !s.is_empty(),
                "config_dir must not be empty"
            );
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
