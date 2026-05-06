use std::path::Path;

/// Abstraction over synchronous filesystem operations.
pub trait Fs: Send + Sync + 'static {
    fn read_to_string(&self, path: &Path) -> std::io::Result<String>;
    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()>;
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()>;
}

// ── Real implementation ───────────────────────────────────────────────────────

pub struct RealFs;

impl Fs for RealFs {
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, contents)
    }
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }
}

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// In-memory filesystem mock. All operations are keyed by the canonical path string.
    pub struct MockFs {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
    }

    impl MockFs {
        pub fn new() -> Self {
            Self { files: Mutex::new(HashMap::new()) }
        }

        pub fn add_file(&self, path: impl Into<PathBuf>, content: impl AsRef<[u8]>) {
            self.files.lock().unwrap().insert(path.into(), content.as_ref().to_vec());
        }

        pub fn read_bytes(&self, path: &Path) -> Option<Vec<u8>> {
            self.files.lock().unwrap().get(path).cloned()
        }

        pub fn contains(&self, path: &Path) -> bool {
            self.files.lock().unwrap().contains_key(path)
        }
    }

    impl Fs for MockFs {
        fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "file not found in MockFs"))
        }

        fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
            self.files.lock().unwrap().insert(path.to_path_buf(), contents.to_vec());
            Ok(())
        }

        fn create_dir_all(&self, _path: &Path) -> std::io::Result<()> {
            Ok(())
        }
    }
}
