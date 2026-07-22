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

    assert_eq!(get_state(&dirs, "myapp"), Some(PathBuf::from("/var/state/myapp")));
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
