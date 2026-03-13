use std::path::PathBuf;

pub trait DataLocalDir {
    fn data_local_dir(&self) -> Option<PathBuf>;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::DataLocalDir;
    use crate::dirs::{DirKind, FixedDirs};

    #[test]
    fn returns_none_when_not_set() {
        let dirs = FixedDirs::new();
        assert!(dirs.data_local_dir().is_none());
    }

    #[test]
    fn returns_configured_path() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::DataLocal, "/home/user/.local/share");

        assert_eq!(
            dirs.data_local_dir(),
            Some(PathBuf::from("/home/user/.local/share"))
        );
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn app_data_local<D: DataLocalDir>(dirs: &D, app: &str) -> Option<PathBuf> {
            dirs.data_local_dir().map(|d| d.join(app))
        }

        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::DataLocal, "/local/share");

        assert_eq!(
            app_data_local(&dirs, "myapp"),
            Some(PathBuf::from("/local/share/myapp"))
        );
        assert_eq!(app_data_local(&FixedDirs::new(), "myapp"), None);
    }
}
