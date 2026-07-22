use std::path::PathBuf;

pub trait DataLocalDir {
    fn data_local_dir(&self) -> Option<PathBuf>;
}
