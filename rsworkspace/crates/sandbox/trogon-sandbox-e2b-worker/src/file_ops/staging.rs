use std::fs::File;
use std::future::Future;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use tempfile::TempDir;
use trogon_sandbox_contract::{FileOp, FileOpErrorKind};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::constants::{COLLECTION_DEADLINE, MAX_RELATIVE_PATH_BYTES, MAX_RELATIVE_PATH_SEGMENTS};
use super::file_guest::FileGuest;

/// The failure a host staging step that hit an I/O error stands for.
pub(super) fn staging_failure(error: std::io::Error) -> FileOpError {
    FileOpError::failed(
        FileOpErrorKind::Io,
        format!("file operation staging failed: {}", error.kind()),
    )
}

/// Refuses a path that is empty, oversized, or not a plain relative path.
pub(super) fn check_relative(path: &str) -> Result<(), FileOpError> {
    if path.is_empty()
        || path.len() > MAX_RELATIVE_PATH_BYTES
        || path.contains('\0')
        || path.split('/').count() > MAX_RELATIVE_PATH_SEGMENTS
        || !Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(FileOpError::invalid_path("invalid or oversized relative file path"));
    }
    Ok(())
}

/// Runs `work` against a fresh host staging directory under the collection deadline, then removes the directory.
pub(super) async fn with_staging<T, F, Fut>(work: F) -> Result<T, FileOpError>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = Result<T, FileOpError>>,
{
    let directory = TempDir::new().map_err(staging_failure)?;
    let result = tokio::time::timeout(COLLECTION_DEADLINE, work(directory.path().to_path_buf()))
        .await
        .map_err(|_| FileOpError::failed(FileOpErrorKind::Io, "file collection exceeded its deadline"))
        .and_then(|result| result);
    let closed = directory.close();
    let value = result?;
    closed.map_err(|_| FileOpError::failed(FileOpErrorKind::Io, "host staging cleanup failed"))?;
    Ok(value)
}

/// Creates the host file `host` and any missing directories above it.
pub(super) fn create_staged(host: &Path) -> Result<File, FileOpError> {
    host.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .map_err(staging_failure)?;
    File::create(host).map_err(staging_failure)
}

/// Copies `size` bytes of the guest file `remote` into the host file `host`, one read chunk at a time.
pub(super) async fn download(guest: &FileGuest<'_>, remote: &str, host: &Path, size: u64) -> Result<(), FileOpError> {
    let mut file = create_staged(host)?;
    let mut offset = 0;
    while offset < size {
        let count = (size - offset).min(FileOp::MAX_READ_BYTES);
        let bytes = guest
            .envd
            .read_file_range(guest.target, guest.user, remote, offset, count)
            .await
            .map_err(|error| api_failure(error, remote))?
            .ok_or_else(|| FileOpError::NotFound { path: remote.into() })?;
        if bytes.len() as u64 != count {
            return Err(FileOpError::failed(
                FileOpErrorKind::Io,
                "file changed during collection",
            ));
        }
        file.write_all(&bytes).map_err(staging_failure)?;
        offset += count;
    }
    Ok(())
}
