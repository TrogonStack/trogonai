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
