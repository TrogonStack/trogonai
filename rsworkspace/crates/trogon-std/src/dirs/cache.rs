use std::path::PathBuf;

pub trait CacheDir {
    fn cache_dir(&self) -> Option<PathBuf>;
}
