#[cfg(any(test, feature = "test-support"))]
use std::cell::RefCell;
#[cfg(any(test, feature = "test-support"))]
use std::collections::{HashMap, HashSet};
#[cfg(any(test, feature = "test-support"))]
use std::io;
#[cfg(any(test, feature = "test-support"))]
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "test-support"))]
use std::sync::{Arc, Mutex};

#[cfg(any(test, feature = "test-support"))]
use super::{CreateDirAll, ExistsFile, OpenAppendFile, ReadFile, WriteFile};

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
    files: Arc<Mutex<HashMap<PathBuf, String>>>,
    dirs: RefCell<HashSet<PathBuf>>,
    opened_files: RefCell<HashSet<PathBuf>>,
}

#[cfg(any(test, feature = "test-support"))]
pub struct MemAppendWriter {
    path: PathBuf,
    files: Arc<Mutex<HashMap<PathBuf, String>>>,
}

#[cfg(any(test, feature = "test-support"))]
impl MemFs {
    pub fn new() -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            dirs: RefCell::new(HashSet::new()),
            opened_files: RefCell::new(HashSet::new()),
        }
    }

    pub fn insert(&self, path: impl AsRef<Path>, content: impl Into<String>) {
        self.files
            .lock()
            .unwrap()
            .insert(path.as_ref().to_path_buf(), content.into());
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.lock().unwrap().keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.files.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.lock().unwrap().is_empty()
    }

    pub fn dir_exists(&self, path: &Path) -> bool {
        self.dirs.borrow().contains(path)
    }

    pub fn was_opened(&self, path: &Path) -> bool {
        self.opened_files.borrow().contains(path)
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
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))
    }
}

#[cfg(any(test, feature = "test-support"))]
impl WriteFile for MemFs {
    fn write(&self, path: &Path, contents: &str) -> io::Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ExistsFile for MemFs {
    fn exists(&self, path: &Path) -> bool {
        self.files.lock().unwrap().contains_key(path)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl io::Write for MemAppendWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s =
            std::str::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.files
            .lock()
            .unwrap()
            .entry(self.path.clone())
            .or_default()
            .push_str(s);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl OpenAppendFile for MemFs {
    type Writer = MemAppendWriter;

    fn open_append(&self, path: &Path) -> io::Result<Self::Writer> {
        self.opened_files.borrow_mut().insert(path.to_path_buf());
        Ok(MemAppendWriter {
            path: path.to_path_buf(),
            files: Arc::clone(&self.files),
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl CreateDirAll for MemFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        let files = self.files.lock().unwrap();
        let mut dirs = self.dirs.borrow_mut();
        for ancestor in path.ancestors() {
            if ancestor.as_os_str().is_empty() {
                break;
            }
            if files.contains_key(ancestor) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("path component is a file: {}", ancestor.display()),
                ));
            }
            dirs.insert(ancestor.to_path_buf());
        }
        Ok(())
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
    fn test_memfs_open_append_persists_writes() {
        use std::io::Write;

        let fs = MemFs::new();
        let path = Path::new("/log.txt");

        let mut w = fs.open_append(path).unwrap();
        w.write_all(b"line1\n").unwrap();
        w.write_all(b"line2\n").unwrap();
        drop(w);

        assert_eq!(fs.read_to_string(path).unwrap(), "line1\nline2\n");
    }

    #[test]
    fn test_memfs_open_append_to_existing_file() {
        use std::io::Write;

        let fs = MemFs::new();
        let path = Path::new("/log.txt");
        fs.insert(path, "existing\n");

        let mut w = fs.open_append(path).unwrap();
        w.write_all(b"appended\n").unwrap();
        drop(w);

        assert_eq!(fs.read_to_string(path).unwrap(), "existing\nappended\n");
    }

    #[test]
    fn test_memfs_create_dir_all() {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/a/b/c")).unwrap();

        assert!(fs.dir_exists(Path::new("/a/b/c")));
        assert!(fs.dir_exists(Path::new("/a/b")));
        assert!(fs.dir_exists(Path::new("/a")));
        assert!(fs.dir_exists(Path::new("/")));
    }

    #[test]
    fn test_memfs_create_dir_all_fails_when_component_is_file() {
        let fs = MemFs::new();
        fs.insert("/a/b", "file content");

        let err = fs.create_dir_all(Path::new("/a/b/c")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn test_memfs_create_dir_all_fails_when_target_is_file() {
        let fs = MemFs::new();
        fs.insert("/x", "data");

        let err = fs.create_dir_all(Path::new("/x")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn test_memfs_create_dir_all_idempotent() {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/x/y")).unwrap();
        fs.create_dir_all(Path::new("/x/y")).unwrap();

        assert!(fs.dir_exists(Path::new("/x/y")));
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
