use std::ops::Deref;
use std::path::{Path, PathBuf};

use super::extract_files::set_mode;

/// A path a scanner reads. An extracted tree is read-only on disk, so dropping it restores write access for the
/// directory cleanup that follows.
#[derive(Debug)]
pub struct ScanInput {
    path: PathBuf,
    tree: bool,
}

impl ScanInput {
    pub(crate) fn file(path: PathBuf) -> Self {
        Self { path, tree: false }
    }

    pub(super) fn tree(path: PathBuf) -> Self {
        Self { path, tree: true }
    }
}

impl Deref for ScanInput {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScanInput {
    fn drop(&mut self) {
        if !self.tree {
            return;
        }
        let mut pending = vec![self.path.clone()];
        while let Some(path) = pending.pop() {
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            let _ = set_mode(&path, 0o700);
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        pending.push(entry.path());
                    }
                }
            }
        }
    }
}
