use trogon_sandbox_contract::{FileKind, FileOp, FileOpErrorKind, FileOpResponse, FileReadEnd, FileResultTruncation};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::file_guest::FileGuest;
use super::payload::encode;

pub(super) async fn read(
    guest: &FileGuest<'_>,
    path: &str,
    offset: u64,
    length: Option<u64>,
    max_bytes: Option<u64>,
) -> Result<FileOpResponse, FileOpError> {
    let metadata = guest.stat(path).await?;
    if metadata.kind == FileKind::Directory {
        return Err(FileOpError::failed(
            FileOpErrorKind::IsADirectory,
            format!("{path} is a directory"),
        ));
    }
    let total_size = metadata.size;
    let available = total_size.saturating_sub(offset);
    let wanted = length.map_or(available, |length| length.min(available));
    let count = wanted.min(max_bytes.unwrap_or(FileOp::MAX_READ_BYTES).min(FileOp::MAX_READ_BYTES));
    let data = guest
        .envd
        .read_file_range(guest.target, guest.user, path, offset, count)
        .await
        .map_err(|error| api_failure(error, path))?
        .ok_or_else(|| FileOpError::NotFound { path: path.into() })?;
    Ok(FileOpResponse::Content {
        eof: FileReadEnd::from(offset.saturating_add(data.len() as u64) >= total_size),
        truncated: FileResultTruncation::from(count < wanted),
        data: encode(&data)?,
        offset,
        total_size,
    })
}
