use std::collections::BTreeMap;

use trogon_sandbox_contract::{FileContentHashing, FileRecursion};
use trogon_sandbox_file_ops::{FileOpError, Manifest, ManifestEntry, content_hash};

use crate::constants::STANDARD_DIRS;
use crate::resolve;

/// A path-keyed file map. Directories are implicit: writes create their
/// parents for free.
#[derive(Debug, Default, Clone)]
pub struct MemoryFs {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemoryFs {
    /// Stores `content` at `path`, replacing what was there.
    pub fn write(&mut self, path: &str, content: Vec<u8>) {
        self.files.insert(resolve("/", path), content);
    }

    /// The bytes stored at `path`.
    pub fn read(&self, path: &str) -> Option<&[u8]> {
        self.files.get(&resolve("/", path)).map(Vec::as_slice)
    }

    /// Whether a file is stored at `path`.
    pub fn exists(&self, path: &str) -> bool {
        self.read(path).is_some()
    }

    /// Whether `path` is a directory: the root, one of the directories every
    /// sandbox image has, or a parent of some file.
    pub fn is_dir(&self, path: &str) -> bool {
        let path = resolve("/", path);
        if path == "/" || STANDARD_DIRS.contains(&path.as_str()) {
            return true;
        }
        let prefix = format!("{path}/");
        self.files
            .range(prefix.clone()..)
            .next()
            .is_some_and(|(stored, _)| stored.starts_with(&prefix))
    }

    /// Fingerprints the files at or under `path`. Directories are implicit in this map, so
    /// only files are entries, and the content hash is always present: a change is exactly a
    /// different byte sequence, never a clock tick.
    pub fn manifest(
        &self,
        path: &str,
        recursion: FileRecursion,
        _hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpError> {
        let base = resolve("/", path);
        let prefix = if base == "/" {
            "/".to_string()
        } else {
            format!("{base}/")
        };
        let mut manifest = Manifest::new();
        for (stored, body) in &self.files {
            let included = *stored == base
                || stored
                    .strip_prefix(&prefix)
                    .is_some_and(|rest| recursion.is_recursive() || !rest.contains('/'));
            if included {
                let size = u64::try_from(body.len()).unwrap_or(u64::MAX);
                manifest.insert(
                    stored.clone(),
                    ManifestEntry::file(size, None, Some(content_hash(body)?)),
                );
            }
        }
        Ok(manifest)
    }

    /// How many files are stored.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether no file is stored.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}
