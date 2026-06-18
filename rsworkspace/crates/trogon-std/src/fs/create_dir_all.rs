use std::io;
use std::path::Path;

pub trait CreateDirAll {
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
}
