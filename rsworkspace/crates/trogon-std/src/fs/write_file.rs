use std::io;
use std::path::Path;

pub trait WriteFile {
    fn write(&self, path: &Path, contents: &str) -> io::Result<()>;
}
