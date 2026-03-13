use std::path::PathBuf;

pub trait DataDir {
    fn data_dir(&self) -> Option<PathBuf>;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::DataDir;
    use crate::dirs::{DirKind, FixedDirs};

    #[test]
    fn returns_none_when_not_set() {
        let dirs = FixedDirs::new();
        assert!(dirs.data_dir().is_none());
    }

    #[test]
    fn returns_configured_path() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Data, "/usr/share/myapp");

        assert_eq!(dirs.data_dir(), Some(PathBuf::from("/usr/share/myapp")));
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn data_path<D: DataDir>(dirs: &D, app: &str) -> Option<PathBuf> {
            dirs.data_dir().map(|d| d.join(app))
        }

        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Data, "/data");

        assert_eq!(
            data_path(&dirs, "myapp"),
            Some(PathBuf::from("/data/myapp"))
        );
        assert_eq!(data_path(&FixedDirs::new(), "myapp"), None);
    }
}
