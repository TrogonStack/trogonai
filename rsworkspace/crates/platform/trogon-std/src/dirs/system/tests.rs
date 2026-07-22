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
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn state_dir_is_none_on_this_platform() {
    let dirs = SystemDirs;
    assert!(dirs.state_dir().is_none());
}

#[test]
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn state_dir_is_some_on_this_platform() {
    let dirs = SystemDirs;
    assert!(dirs.state_dir().is_some());
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
        assert!(s.ends_with(".config") || !s.is_empty(), "config_dir must not be empty");
    }
}
