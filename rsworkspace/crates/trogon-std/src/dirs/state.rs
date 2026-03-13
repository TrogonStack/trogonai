use std::path::PathBuf;

pub trait StateDir {
    fn state_dir(&self) -> Option<PathBuf>;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::StateDir;
    use crate::dirs::{DirKind, FixedDirs};

    #[test]
    fn returns_none_when_not_set() {
        let dirs = FixedDirs::new();
        assert!(dirs.state_dir().is_none());
    }

    #[test]
    fn returns_configured_path() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::State, "/var/lib/myapp");

        assert_eq!(dirs.state_dir(), Some(PathBuf::from("/var/lib/myapp")));
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn state_file<D: StateDir>(dirs: &D, name: &str) -> Option<PathBuf> {
            dirs.state_dir().map(|d| d.join(name))
        }

        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::State, "/state");

        assert_eq!(
            state_file(&dirs, "lock"),
            Some(PathBuf::from("/state/lock"))
        );
        assert_eq!(state_file(&FixedDirs::new(), "lock"), None);
    }
}
