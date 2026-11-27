use trogon_sandbox_contract::{FileKind, FileOp, FileOpErrorKind, FileRecursion, FileStat};
use trogon_sandbox_file_ops::FileOpError;

use super::constants::MAX_SYMLINK_HOPS;
use super::file_guest::FileGuest;
use super::list::list_entries;
use super::staging::check_relative;

/// What a search or archive collects: a file, or a directory tree listed from its root.
pub(super) struct CollectionSource {
    pub(super) root: FileStat,
    pub(super) entries: Vec<FileStat>,
    pub(super) truncated: bool,
}

/// Follows symlinks from `path` to the entry they end at.
pub(super) async fn resolve_root(guest: &FileGuest<'_>, path: &str) -> Result<FileStat, FileOpError> {
    let mut current = path.to_string();
    for _ in 0..MAX_SYMLINK_HOPS {
        let metadata = guest.stat(&current).await?;
        if metadata.kind != FileKind::Symlink {
            return Ok(metadata);
        }
        let destination = metadata
            .symlink_target
            .ok_or_else(|| FileOpError::failed(FileOpErrorKind::Io, "envd omitted the symlink target"))?;
        if destination.contains('\0') {
            return Err(FileOpError::invalid_path("symlink target contains NUL"));
        }
        current = if destination.starts_with('/') {
            destination
        } else {
            let (parent, _) = metadata.path.rsplit_once('/').unwrap_or((".", &metadata.path));
            format!("{parent}/{destination}")
        };
    }
    Err(FileOpError::invalid_path("too many symlinks at the selected root"))
}

impl CollectionSource {
    pub(super) async fn load(guest: &FileGuest<'_>, path: &str) -> Result<Self, FileOpError> {
        let root = resolve_root(guest, path).await?;
        let (entries, truncated) = match root.kind {
            FileKind::Directory => {
                list_entries(guest, &root.path, FileRecursion::Recursive, Some(FileOp::MAX_ENTRIES)).await?
            }
            FileKind::File => {
                let mut entry = root.clone();
                entry.path = root.name.clone();
                (vec![entry], false)
            }
            FileKind::Symlink | FileKind::Other => {
                return Err(FileOpError::invalid_request(
                    "selected path is not a regular file or directory",
                ));
            }
        };
        for entry in &entries {
            check_relative(&entry.path)?;
        }
        Ok(Self {
            root,
            entries,
            truncated,
        })
    }

    /// The guest path of one collected entry.
    pub(super) fn remote(&self, entry: &FileStat) -> String {
        if self.root.kind == FileKind::Directory {
            format!("{}/{}", self.root.path.trim_end_matches('/'), entry.path)
        } else {
            self.root.path.clone()
        }
    }
}
