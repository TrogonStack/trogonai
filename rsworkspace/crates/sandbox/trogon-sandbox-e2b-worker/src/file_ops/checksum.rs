use trogon_sandbox_contract::{FileKind, FileOpErrorKind, FileOpResponse};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::file_guest::FileGuest;

pub(super) async fn checksum(
    guest: &FileGuest<'_>,
    path: &str,
    offset: u64,
    length: Option<u64>,
) -> Result<FileOpResponse, FileOpError> {
    let metadata = guest.stat(path).await?;
    if metadata.kind == FileKind::Directory {
        return Err(FileOpError::failed(
            FileOpErrorKind::IsADirectory,
            "cannot checksum a directory",
        ));
    }
    let available = metadata.size.saturating_sub(offset);
    let count = length.map_or(available, |length| length.min(available));
    let (sha256, bytes) = guest
        .envd
        .hash_file_range(guest.target, guest.user, path, offset, count)
        .await
        .map_err(|error| api_failure(error, path))?
        .ok_or_else(|| FileOpError::NotFound { path: path.into() })?;
    Ok(FileOpResponse::Checksum {
        sha256,
        bytes,
        total_size: metadata.size,
    })
}
