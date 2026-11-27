use trogon_sandbox_contract::{FileKind, FileOp, FileOpResponse, FileResultTruncation};
use trogon_sandbox_file_ops::{FileOpError, FsRoot, GlobPattern};

use super::collection_source::CollectionSource;
use super::constants::MAX_SEARCH_BYTES;
use super::file_guest::FileGuest;
use super::search_spec::SearchSpec;
use super::staging::{create_staged, download, staging_failure, with_staging};

/// `reply` marked truncated when the collection that fed it was, so a search never reports a cut-short scan as complete.
pub(super) fn with_truncation(reply: FileOpResponse, collection_truncated: bool) -> FileOpResponse {
    match reply {
        FileOpResponse::SearchResults {
            matches,
            truncated,
            files_scanned,
        } => FileOpResponse::SearchResults {
            matches,
            truncated: FileResultTruncation::from(truncated.is_truncated() || collection_truncated),
            files_scanned,
        },
        other => other,
    }
}

pub(super) async fn search(guest: &FileGuest<'_>, spec: &SearchSpec<'_>) -> Result<FileOpResponse, FileOpError> {
    with_staging(|directory| async move {
        let filesystem = FsRoot::new(&directory, None).map_err(staging_failure)?;
        filesystem.try_execute(&spec.staged)?;
        let matcher = spec
            .glob
            .map(GlobPattern::new)
            .transpose()
            .map_err(|error| FileOpError::invalid_request(error.to_string()))?;
        let bare = spec.glob.is_some_and(|glob| !glob.contains('/'));
        let files = CollectionSource::load(guest, spec.path).await?;
        let mut bytes = 0u64;
        let mut truncated = files.truncated;
        let cap = spec.max_file_bytes.unwrap_or(FileOp::DEFAULT_SEARCH_FILE_BYTES);
        for entry in &files.entries {
            if entry.kind != FileKind::File {
                continue;
            }
            let subject = if bare {
                entry.path.rsplit('/').next().unwrap_or(&entry.path)
            } else {
                &entry.path
            };
            if matcher.as_ref().is_some_and(|matcher| !matcher.is_match(subject)) {
                continue;
            }
            let host = directory.join(&entry.path);
            if !spec.searches_content {
                create_staged(&host)?;
                continue;
            }
            if entry.size > cap {
                continue;
            }
            bytes = bytes.saturating_add(entry.size);
            if bytes > MAX_SEARCH_BYTES {
                truncated = true;
                break;
            }
            download(guest, &files.remote(entry), &host, entry.size).await?;
        }
        Ok(with_truncation(filesystem.try_execute(&spec.staged)?, truncated))
    })
    .await
}
