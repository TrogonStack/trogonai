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
mod tests {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn test_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("trogon_std_system_fs_{name}_{}_{}", std::process::id(), nanos))
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(path);
    }

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
    fn write_read_and_exists_roundtrip() {
        let fs = SystemFs;
        let dir = test_path("roundtrip");
        let path = dir.join("config.json");
        cleanup(&dir);

        std::fs::create_dir_all(&dir).expect("test dir should be created");
        assert!(!fs.exists(&path));

        fs.write(&path, r#"{"port":8080}"#).expect("write should succeed");
        assert!(fs.exists(&path));
        assert_eq!(
            fs.read_to_string(&path).expect("read should succeed"),
            r#"{"port":8080}"#
        );

        cleanup(&dir);
    }

    /// `WriteFile::write` overwrites existing content entirely.
    #[test]
    fn write_overwrites_existing_content() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        let fs = SystemFs;

        fs.write(path, "first").unwrap();
        fs.write(path, "second").unwrap();
        assert_eq!(fs.read_to_string(path).unwrap(), "second");
    }

    #[test]
    fn create_dir_all_creates_nested_directories() {
        let fs = SystemFs;
        let dir = test_path("nested");
        let nested = dir.join("a/b/c");
        cleanup(&dir);

        fs.create_dir_all(&nested).expect("create_dir_all should succeed");
        assert!(nested.exists());

        cleanup(&dir);
    }

    /// `CreateDirAll::create_dir_all` is idempotent — calling it on an
    /// already-existing directory must not return an error.
    #[test]
    fn create_dir_all_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("existing");
        let fs = SystemFs;

        fs.create_dir_all(&dir).unwrap();
        fs.create_dir_all(&dir).unwrap();
    }

    #[test]
    fn create_dir_all_fails_when_path_is_existing_file() {
        let fs = SystemFs;
        let dir = test_path("create_dir_error");
        let file = dir.join("existing_file");
        cleanup(&dir);

        std::fs::create_dir_all(&dir).expect("test dir should be created");
        std::fs::write(&file, "x").expect("test file should be created");

        assert!(fs.create_dir_all(&file).is_err());

        cleanup(&dir);
    }

    #[test]
    fn open_append_creates_and_appends_file() {
        let fs = SystemFs;
        let dir = test_path("append");
        let file = dir.join("log.txt");
        cleanup(&dir);

        std::fs::create_dir_all(&dir).expect("test dir should be created");

        let mut w1 = fs.open_append(&file).expect("first open should succeed");
        w1.write_all(b"hello").expect("first write should succeed");
        drop(w1);

        let mut w2 = fs.open_append(&file).expect("second open should succeed");
        w2.write_all(b" world").expect("second write should succeed");
        drop(w2);

        assert_eq!(
            std::fs::read_to_string(&file).expect("file should be readable"),
            "hello world"
        );

        cleanup(&dir);
    }

    /// `OpenAppendFile::open_append` appends to an existing file rather than
    /// truncating it.
    #[test]
    fn open_append_does_not_truncate_existing_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        let fs = SystemFs;

        fs.write(path, "existing\n").unwrap();

        let mut w = fs.open_append(path).unwrap();
        w.write_all(b"appended\n").unwrap();
        drop(w);

        assert_eq!(fs.read_to_string(path).unwrap(), "existing\nappended\n");
    }

    #[test]
    fn open_append_fails_when_parent_directory_is_missing() {
        let fs = SystemFs;
        let dir = test_path("append_missing_parent");
        let file = dir.join("subdir/log.txt");
        cleanup(&dir);

        assert!(fs.open_append(&file).is_err());
    }
}
