use std::path::PathBuf;

pub trait ConfigDir {
    fn config_dir(&self) -> Option<PathBuf>;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ConfigDir;
    use crate::dirs::{DirKind, FixedDirs};

    #[test]
    fn returns_none_when_not_set() {
        let dirs = FixedDirs::new();
        assert!(dirs.config_dir().is_none());
    }

    #[test]
    fn returns_configured_path() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Config, "/etc/myapp");

        assert_eq!(dirs.config_dir(), Some(PathBuf::from("/etc/myapp")));
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn config_file<D: ConfigDir>(dirs: &D, name: &str) -> Option<PathBuf> {
            dirs.config_dir().map(|d| d.join(name))
        }

        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Config, "/config");

        assert_eq!(
            config_file(&dirs, "settings.toml"),
            Some(PathBuf::from("/config/settings.toml"))
        );
        assert_eq!(config_file(&FixedDirs::new(), "settings.toml"), None);
    }
}
