use std::path::{Path, PathBuf};

use crate::{ManifestKey, VolumeError};

/// Every regular file under `root` with its key, sorted by key. Symlinks are not followed.
pub(crate) fn tree_files(root: &Path) -> Result<Vec<(ManifestKey, PathBuf)>, VolumeError> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                files.push((ManifestKey::from_relative_path(relative)?, path));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}
