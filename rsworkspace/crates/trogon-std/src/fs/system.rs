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

    #[test]
    fn write_creates_file_with_content() {
        let path = std::env::temp_dir().join("trogon_fs_write_test_xk9");
        let _ = std::fs::remove_file(&path);
        let fs = SystemFs;
        fs.write(&path, "hello world").unwrap();
        assert_eq!(fs.read_to_string(&path).unwrap(), "hello world");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn create_dir_all_creates_nested_directories() {
        let base = std::env::temp_dir()
            .join("trogon_fs_mkdir_xk9")
            .join("nested");
        let _ = std::fs::remove_dir_all(base.parent().unwrap());
        let fs = SystemFs;
        fs.create_dir_all(&base).unwrap();
        assert!(base.is_dir());
        let _ = std::fs::remove_dir_all(base.parent().unwrap());
    }

    #[test]
    fn open_append_creates_and_appends_to_file() {
        use std::io::Write;
        let path = std::env::temp_dir().join("trogon_fs_append_xk9");
        let _ = std::fs::remove_file(&path);
        let fs = SystemFs;
        let mut f = fs.open_append(&path).unwrap();
        f.write_all(b"hello").unwrap();
        drop(f);
        let mut f2 = fs.open_append(&path).unwrap();
        f2.write_all(b" world").unwrap();
        drop(f2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
        let _ = std::fs::remove_file(&path);
    }
}
