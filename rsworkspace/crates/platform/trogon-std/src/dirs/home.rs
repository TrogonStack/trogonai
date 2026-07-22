use std::path::PathBuf;

pub trait HomeDir {
    fn home_dir(&self) -> Option<PathBuf>;
}
