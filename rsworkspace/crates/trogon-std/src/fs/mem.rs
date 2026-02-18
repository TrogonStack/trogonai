#[cfg(any(test, feature = "test-support"))]
use std::cell::RefCell;
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "test-support"))]
use std::io;
#[cfg(any(test, feature = "test-support"))]
use std::path::{Path, PathBuf};

#[cfg(any(test, feature = "test-support"))]
use super::{ExistsFile, ReadFile, WriteFile};

/// Uses `RefCell` for interior mutability — all methods take `&self`.
///
/// # Path Semantics
///
/// Paths are stored as raw [`PathBuf`] keys with **no normalization**.
/// `"a.txt"`, `"./a.txt"`, and `"/cwd/a.txt"` are three distinct entries.
/// Be consistent with how you construct paths, or canonicalize before
/// passing them in.
///
/// ```ignore
/// let fs = MemFs::new();
/// fs.insert("config.json", "{}");
///
/// assert!(fs.exists(Path::new("config.json")));   // same literal path
/// assert!(!fs.exists(Path::new("./config.json"))); // different PathBuf
/// ```
#[cfg(any(test, feature = "test-support"))]
pub struct MemFs {
    files: RefCell<HashMap<PathBuf, String>>,
}

#[cfg(any(test, feature = "test-support"))]
impl MemFs {
    pub fn new() -> Self {
        Self {
            files: RefCell::new(HashMap::new()),
        }
    }

    pub fn insert(&self, path: impl AsRef<Path>, content: impl Into<String>) {
        self.files
            .borrow_mut()
            .insert(path.as_ref().to_path_buf(), content.into());
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.borrow().keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.files.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.borrow().is_empty()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ReadFile for MemFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))
    }
}

#[cfg(any(test, feature = "test-support"))]
impl WriteFile for MemFs {
    fn write(&self, path: &Path, contents: &str) -> io::Result<()> {
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ExistsFile for MemFs {
    fn exists(&self, path: &Path) -> bool {
        self.files.borrow().contains_key(path)
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;

    use super::*;

    #[test]
    fn test_memfs_write_and_read() {
        let fs = MemFs::new();
        let path = Path::new("/test/file.txt");

        fs.write(path, "hello world").unwrap();
        let content = fs.read_to_string(path).unwrap();

        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_memfs_exists() {
        let fs = MemFs::new();
        let path = Path::new("/test/file.txt");

        assert!(!fs.exists(path));
        fs.write(path, "content").unwrap();
        assert!(fs.exists(path));
    }

    #[test]
    fn test_memfs_not_found() {
        let fs = MemFs::new();
        let result = fs.read_to_string(Path::new("/nonexistent"));

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn test_memfs_multiple_files() {
        let fs = MemFs::new();

        fs.write(Path::new("/a.txt"), "aaa").unwrap();
        fs.write(Path::new("/b.txt"), "bbb").unwrap();
        fs.write(Path::new("/c.txt"), "ccc").unwrap();

        assert_eq!(fs.read_to_string(Path::new("/a.txt")).unwrap(), "aaa");
        assert_eq!(fs.read_to_string(Path::new("/b.txt")).unwrap(), "bbb");
        assert_eq!(fs.read_to_string(Path::new("/c.txt")).unwrap(), "ccc");
        assert_eq!(fs.len(), 3);
    }

    #[test]
    fn test_memfs_overwrite() {
        let fs = MemFs::new();
        let path = Path::new("/test.txt");

        fs.write(path, "v1").unwrap();
        assert_eq!(fs.read_to_string(path).unwrap(), "v1");

        fs.write(path, "v2").unwrap();
        assert_eq!(fs.read_to_string(path).unwrap(), "v2");

        assert_eq!(fs.len(), 1);
    }

    #[test]
    fn test_memfs_insert_helper() {
        let fs = MemFs::new();
        fs.insert("/config.json", r#"{"key": "value"}"#);

        let content = fs.read_to_string(Path::new("/config.json")).unwrap();
        assert!(content.contains("key"));
    }

    #[test]
    fn test_memfs_empty() {
        let fs = MemFs::new();
        assert!(fs.is_empty());
        assert_eq!(fs.len(), 0);

        fs.insert("/file.txt", "data");
        assert!(!fs.is_empty());
        assert_eq!(fs.len(), 1);
    }

    #[test]
    fn test_memfs_default() {
        let fs = MemFs::default();
        assert!(fs.is_empty());
    }

    #[test]
    fn test_memfs_unicode_content() {
        let fs = MemFs::new();
        let path = Path::new("/unicode.txt");
        let content = "Hello 世界 🌍 café";

        fs.write(path, content).unwrap();
        assert_eq!(fs.read_to_string(path).unwrap(), content);
    }

    #[test]
    fn test_memfs_empty_content() {
        let fs = MemFs::new();
        let path = Path::new("/empty.txt");

        fs.write(path, "").unwrap();
        assert!(fs.exists(path));
        assert_eq!(fs.read_to_string(path).unwrap(), "");
    }

    #[test]
    fn test_memfs_nested_paths() {
        let fs = MemFs::new();

        fs.insert("/a/b/c/deep.txt", "deep");
        fs.insert("/a/b/shallow.txt", "shallow");

        assert_eq!(
            fs.read_to_string(Path::new("/a/b/c/deep.txt")).unwrap(),
            "deep"
        );
        assert_eq!(
            fs.read_to_string(Path::new("/a/b/shallow.txt")).unwrap(),
            "shallow"
        );
    }

    #[test]
    fn test_generic_function_with_memfs() {
        use super::super::ReadFile;

        fn read_config<F: ReadFile>(fs: &F, path: &Path) -> String {
            fs.read_to_string(path).unwrap_or_else(|_| "{}".to_string())
        }

        let fs = MemFs::new();
        fs.insert("/config.json", r#"{"port": 8080}"#);

        assert_eq!(
            read_config(&fs, Path::new("/config.json")),
            r#"{"port": 8080}"#
        );
        assert_eq!(read_config(&fs, Path::new("/missing.json")), "{}");
    }
}
