use std::path::PathBuf;

pub trait StateDir {
    fn state_dir(&self) -> Option<PathBuf>;
}
