use std::path::PathBuf;

pub trait CacheDir {
    fn cache_dir(&self) -> Option<PathBuf>;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::CacheDir;
    use crate::dirs::{DirKind, FixedDirs};

    #[test]
    fn returns_none_when_not_set() {
        let dirs = FixedDirs::new();
        assert!(dirs.cache_dir().is_none());
    }

    #[test]
    fn returns_configured_path() {
        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Cache, "/tmp/cache");

        assert_eq!(dirs.cache_dir(), Some(PathBuf::from("/tmp/cache")));
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn cache_file<D: CacheDir>(dirs: &D, name: &str) -> Option<PathBuf> {
            dirs.cache_dir().map(|d| d.join(name))
        }

        let mut dirs = FixedDirs::new();
        dirs.set(DirKind::Cache, "/cache");

        assert_eq!(
            cache_file(&dirs, "index.bin"),
            Some(PathBuf::from("/cache/index.bin"))
        );
        assert_eq!(cache_file(&FixedDirs::new(), "index.bin"), None);
    }
}
