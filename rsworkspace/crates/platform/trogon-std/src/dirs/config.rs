use std::path::PathBuf;

pub trait ConfigDir {
    fn config_dir(&self) -> Option<PathBuf>;
}
