use std::path::PathBuf;

pub trait DataDir {
    fn data_dir(&self) -> Option<PathBuf>;
}
