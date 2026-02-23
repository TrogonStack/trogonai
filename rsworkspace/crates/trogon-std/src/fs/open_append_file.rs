use std::io;
use std::path::Path;

pub trait OpenAppendFile {
    type Writer: io::Write + Send + 'static;
    fn open_append(&self, path: &Path) -> io::Result<Self::Writer>;
}
