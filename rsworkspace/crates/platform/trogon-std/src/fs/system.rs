use std::path::Path;

use super::{CreateDirAll, ExistsFile, OpenAppendFile, ReadFile, WriteFile};

/// Zero-sized type — delegates to `std::fs`.
pub struct SystemFs;

impl ReadFile for SystemFs {
    #[inline]
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
}

impl WriteFile for SystemFs {
    #[inline]
    fn write(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        std::fs::write(path, contents)
    }
}

impl ExistsFile for SystemFs {
    #[inline]
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

impl CreateDirAll for SystemFs {
    #[inline]
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }
}

impl OpenAppendFile for SystemFs {
    type Writer = std::fs::File;

    #[inline]
    fn open_append(&self, path: &Path) -> std::io::Result<Self::Writer> {
        std::fs::File::options().create(true).append(true).open(path)
    }
}

#[cfg(test)]
mod tests;
