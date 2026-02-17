use std::io;
use std::path::Path;

pub trait ReadFile {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
}
