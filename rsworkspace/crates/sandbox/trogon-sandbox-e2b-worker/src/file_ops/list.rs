use trogon_sandbox_contract::{
    FileKind, FileOp, FileOpErrorKind, FileOpResponse, FileRecursion, FileResultTruncation, FileStat,
};
use trogon_sandbox_file_ops::FileOpError;

use super::file_guest::FileGuest;

/// The entries under a directory, relative to it, and whether the listing hit its cap.
pub(super) async fn list_entries(
    guest: &FileGuest<'_>,
    path: &str,
    recursion: FileRecursion,
    max_entries: Option<u32>,
) -> Result<(Vec<FileStat>, bool), FileOpError> {
    let root = guest.stat(path).await?;
    if !matches!(root.kind, FileKind::Directory | FileKind::Symlink) {
        return Err(FileOpError::failed(
            FileOpErrorKind::NotADirectory,
            format!("{path} is not a directory"),
        ));
    }
    let prefix = format!("{}/", root.path.trim_end_matches('/'));
    let limit = max_entries
        .unwrap_or(FileOp::DEFAULT_MAX_ENTRIES)
        .clamp(1, FileOp::MAX_ENTRIES) as usize;
    let mut pending = vec![guest.directory(&root.path).await?.into_iter()];
    let mut entries = Vec::new();
    let mut truncated = false;
    while let Some(children) = pending.last_mut() {
        let Some(mut entry) = children.next() else {
            pending.pop();
            continue;
        };
        if entries.len() == limit {
            truncated = true;
            break;
        }
        let absolute = entry.path.clone();
        entry.path = absolute
            .strip_prefix(&prefix)
            .ok_or_else(|| FileOpError::failed(FileOpErrorKind::Io, "envd returned an entry outside the listing root"))?
            .to_string();
        let descend = recursion.is_recursive() && entry.kind == FileKind::Directory;
        entries.push(entry);
        if descend {
            pending.push(guest.directory(&absolute).await?.into_iter());
        }
    }
    Ok((entries, truncated))
}

pub(super) async fn list(
    guest: &FileGuest<'_>,
    path: &str,
    recursion: FileRecursion,
    max_entries: Option<u32>,
) -> Result<FileOpResponse, FileOpError> {
    let (entries, truncated) = list_entries(guest, path, recursion, max_entries).await?;
    Ok(FileOpResponse::Listing {
        entries,
        truncated: FileResultTruncation::from(truncated),
    })
}
