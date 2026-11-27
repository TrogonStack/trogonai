use crate::ulid::Ulid;
use trogon_sandbox_contract::{FileKind, FileOp, FileOpErrorKind, FileOpResponse};
use trogon_sandbox_file_ops::{EditOutcome, EditSpec, FileOpError};

use super::api_failure::api_failure;
use super::constants::{EDIT, EXIT_CHANGED, SCRIPT_TIMEOUT};
use super::file_guest::FileGuest;
use super::script_failure::script_failure;
use super::write::split_parent;

pub(super) async fn edit(
    guest: &FileGuest<'_>,
    path: &str,
    find: &str,
    replace: &str,
    expected_occurrences: u32,
) -> Result<FileOpResponse, FileOpError> {
    let spec = EditSpec::new(find, replace, expected_occurrences)?;
    let metadata = guest.stat(path).await?;
    if metadata.kind == FileKind::Directory {
        return Err(FileOpError::failed(
            FileOpErrorKind::IsADirectory,
            "cannot edit a directory",
        ));
    }
    if metadata.size > FileOp::MAX_READ_BYTES {
        return Err(FileOpError::too_large("file exceeds the edit byte limit"));
    }
    let original = guest
        .envd
        .read_file_range(guest.target, guest.user, path, 0, FileOp::MAX_READ_BYTES)
        .await
        .map_err(|error| api_failure(error, path))?
        .ok_or_else(|| FileOpError::NotFound { path: path.into() })?;
    let text =
        std::str::from_utf8(&original).map_err(|_| FileOpError::invalid_request("file is not valid UTF-8 text"))?;
    let occurrences = u32::try_from(text.matches(find).count()).unwrap_or(u32::MAX);
    if occurrences == expected_occurrences {
        let final_size = (original.len() as u64)
            .checked_sub((find.len() as u64).saturating_mul(u64::from(occurrences)))
            .and_then(|size| size.checked_add((replace.len() as u64).checked_mul(u64::from(occurrences))?));
        if final_size.is_none_or(|size| size > FileOp::MAX_WRITE_BYTES) {
            return Err(FileOpError::too_large("replacement exceeds the edit byte limit"));
        }
    }
    let EditOutcome::Replaced { content, occurrences } = spec.apply(&original)? else {
        return Ok(FileOpResponse::EditRejected {
            occurrences,
            expected: expected_occurrences,
        });
    };
    let size = content.len() as u64;
    let (parent, _) = split_parent(path);
    let prefix = format!("{parent}/.trogon-edit-{}", Ulid::generate());
    let original_path = format!("{prefix}.original");
    let replacement_path = format!("{prefix}.replacement");
    let outcome = async {
        guest
            .envd
            .write_file(guest.target, guest.user, &original_path, original)
            .await
            .map_err(|error| api_failure(error, path))?;
        guest
            .envd
            .write_file(guest.target, guest.user, &replacement_path, content)
            .await
            .map_err(|error| api_failure(error, path))?;
        let args = vec![path.to_string(), original_path.clone(), replacement_path.clone()];
        guest.script(EDIT, args, SCRIPT_TIMEOUT, path).await
    }
    .await;
    guest.discard_logged(&original_path).await;
    guest.discard_logged(&replacement_path).await;
    let output = outcome?;
    if output.exit_code == EXIT_CHANGED {
        return Err(FileOpError::failed(
            FileOpErrorKind::Io,
            "file changed after it was read; edit was not applied",
        ));
    }
    if output.exit_code != 0 {
        return Err(script_failure("file edit", &output));
    }
    Ok(FileOpResponse::Edited { occurrences, size })
}
