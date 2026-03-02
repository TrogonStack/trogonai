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
        std::fs::File::options()
            .create(true)
            .append(true)
            .open(path)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use super::*;

    #[test]
    fn test_system_fs_nonexistent_file() {
        let fs = SystemFs;
        assert!(!fs.exists(Path::new("/nonexistent_trogonstd_test_file_12345")));
    }

    #[test]
    fn test_generic_function_with_system_fs() {
        fn read_config<F: ReadFile>(fs: &F, path: &Path) -> String {
            fs.read_to_string(path).unwrap_or_else(|_| "{}".to_string())
        }

        let fs = SystemFs;
        assert_eq!(read_config(&fs, Path::new("/nonexistent_12345")), "{}");
    }

    /// `WriteFile::write` creates or replaces a file; `ReadFile::read_to_string`
    /// must return the same bytes back.
    #[test]
    fn test_system_fs_write_creates_file_with_content() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        let fs = SystemFs;

        fs.write(path, "hello, world").unwrap();
        assert_eq!(fs.read_to_string(path).unwrap(), "hello, world");
        assert!(fs.exists(path));
    }

    /// `WriteFile::write` overwrites existing content entirely.
    #[test]
    fn test_system_fs_write_overwrites_existing_content() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        let fs = SystemFs;

        fs.write(path, "first").unwrap();
        fs.write(path, "second").unwrap();
        assert_eq!(fs.read_to_string(path).unwrap(), "second");
    }

    /// `CreateDirAll::create_dir_all` must create every component in the path.
    #[test]
    fn test_system_fs_create_dir_all_creates_nested_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        let fs = SystemFs;

        fs.create_dir_all(&nested).unwrap();
        assert!(nested.is_dir());
        assert!(nested.parent().unwrap().is_dir());
    }

    /// `CreateDirAll::create_dir_all` is idempotent — calling it on an
    /// already-existing directory must not return an error.
    #[test]
    fn test_system_fs_create_dir_all_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("existing");
        let fs = SystemFs;

        fs.create_dir_all(&dir).unwrap();
        // Second call on the same directory must succeed.
        fs.create_dir_all(&dir).unwrap();
    }

    /// `OpenAppendFile::open_append` must create the file if it does not
    /// exist and accumulate multiple `write` calls.
    #[test]
    fn test_system_fs_open_append_accumulates_writes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("log.txt");
        let fs = SystemFs;

        let mut w = fs.open_append(&path).unwrap();
        w.write_all(b"line1\n").unwrap();
        w.write_all(b"line2\n").unwrap();
        drop(w);

        assert_eq!(fs.read_to_string(&path).unwrap(), "line1\nline2\n");
    }

    /// `OpenAppendFile::open_append` appends to an existing file rather than
    /// truncating it.
    #[test]
    fn test_system_fs_open_append_does_not_truncate_existing_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        let fs = SystemFs;

        fs.write(path, "existing\n").unwrap();

        let mut w = fs.open_append(path).unwrap();
        w.write_all(b"appended\n").unwrap();
        drop(w);

        assert_eq!(fs.read_to_string(path).unwrap(), "existing\nappended\n");
    }
}
