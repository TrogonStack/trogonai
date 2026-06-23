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
        let s = std::str::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
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
mod tests;
