use std::path::PathBuf;

pub trait HomeDir {
    fn home_dir(&self) -> Option<PathBuf>;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::HomeDir;
    use crate::dirs::{DirKind, FixedDirs};

    #[test]
    fn returns_none_when_not_set() {
        let dirs = FixedDirs::new();
        assert!(dirs.home_dir().is_none());
    }

    #[test]
    fn returns_configured_path() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Home, "/home/alice");

        assert_eq!(dirs.home_dir(), Some(PathBuf::from("/home/alice")));
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn dotfile<D: HomeDir>(dirs: &D, name: &str) -> Option<PathBuf> {
            dirs.home_dir().map(|d| d.join(name))
        }

        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Home, "/home/user");

        assert_eq!(
            dotfile(&dirs, ".bashrc"),
            Some(PathBuf::from("/home/user/.bashrc"))
        );
        assert_eq!(dotfile(&FixedDirs::new(), ".bashrc"), None);
    }
}
