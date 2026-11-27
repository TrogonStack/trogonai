use std::collections::BTreeSet;

use crate::ulid::Ulid;
use trogon_sandbox_contract::{FileKind, FileOpErrorKind, FileOpResponse};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::archive_upload::ArchiveUpload;
use super::collection_source::resolve_root;
use super::constants::{EXTRACT, EXTRACT_TIMEOUT, PREFLIGHT_DEADLINE};
use super::file_guest::FileGuest;
use super::script_failure::script_failure;
use super::write::split_parent;

/// The directory the archive extracts into, after checking that no path it creates passes through a symlink.
async fn preflight_destination(
    guest: &FileGuest<'_>,
    path: &str,
    paths: &BTreeSet<String>,
) -> Result<String, FileOpError> {
    if path.is_empty() || path.contains('\0') {
        return Err(FileOpError::invalid_path(
            "archive destination must be a nonempty path without NUL",
        ));
    }
    let destination = match resolve_root(guest, path).await {
        Ok(metadata) if metadata.kind == FileKind::Directory => metadata.path,
        Ok(_) => {
            return Err(FileOpError::failed(
                FileOpErrorKind::NotADirectory,
                "archive destination is not a directory",
            ));
        }
        Err(FileOpError::NotFound { .. }) => path.to_string(),
        Err(error) => return Err(error),
    };
    for relative in paths {
        match guest
            .stat(&format!("{}/{relative}", destination.trim_end_matches('/')))
            .await
        {
            Ok(metadata) if metadata.kind == FileKind::Symlink => {
                return Err(FileOpError::invalid_path("archive destination contains a symlink"));
            }
            Ok(_) | Err(FileOpError::NotFound { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(destination)
}

pub(super) async fn archive_put(guest: &FileGuest<'_>, path: &str, data: &[u8]) -> Result<FileOpResponse, FileOpError> {
    let ArchiveUpload {
        data,
        paths,
        entries,
        bytes,
    } = ArchiveUpload::canonical(data)?;
    let destination = tokio::time::timeout(PREFLIGHT_DEADLINE, preflight_destination(guest, path, &paths))
        .await
        .map_err(|_| FileOpError::failed(FileOpErrorKind::Io, "archive preflight exceeded its deadline"))??;
    let (parent, _) = split_parent(&destination);
    let temporary = format!("{parent}/.trogon-archive-{}", Ulid::generate());
    let outcome = async {
        guest
            .envd
            .write_file(guest.target, guest.user, &temporary, data)
            .await
            .map_err(|error| api_failure(error, path))?;
        let args = vec![temporary.clone(), destination.clone()];
        guest.script(EXTRACT, args, EXTRACT_TIMEOUT, path).await
    }
    .await;
    let staged = guest.discard(&temporary).await;
    let staged_error = guest.discard(&format!("{temporary}.err")).await;
    let output = outcome?;
    if output.exit_code != 0 {
        return Err(script_failure("archive extraction", &output));
    }
    staged.and(staged_error).map_err(|error| {
        FileOpError::failed(
            FileOpErrorKind::Io,
            format!("archive extracted but upload staging cleanup failed: {error}"),
        )
    })?;
    Ok(FileOpResponse::Extracted { entries, bytes })
}
