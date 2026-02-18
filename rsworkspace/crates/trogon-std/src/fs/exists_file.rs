use std::path::Path;

pub trait ExistsFile {
    fn exists(&self, path: &Path) -> bool;
}
