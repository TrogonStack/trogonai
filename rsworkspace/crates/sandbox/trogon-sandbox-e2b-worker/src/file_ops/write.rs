use crate::ulid::Ulid;
use trogon_sandbox_contract::{FileKind, FileOp, FileOpErrorKind, FileOpResponse};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::constants::{SCRIPT_TIMEOUT, WRITE_SCRIPT};
use super::file_guest::FileGuest;
use super::payload::decode;
use super::script_failure::script_failure;
use super::write_spec::WriteSpec;

/// The directory and file name of `path`, with a bare name addressed against the working directory.
pub(super) fn split_parent(path: &str) -> (&str, &str) {
    let (parent, name) = path.rsplit_once('/').unwrap_or((".", path));
    (if parent.is_empty() { "/" } else { parent }, name)
}

pub(super) async fn write(guest: &FileGuest<'_>, spec: &WriteSpec<'_>) -> Result<FileOpResponse, FileOpError> {
    let WriteSpec {
        path,
        data,
        offset,
        truncate,
        mode,
        create_parents,
    } = spec;
    let data = decode(data);
    if data.len() as u64 > FileOp::MAX_WRITE_BYTES {
        return Err(FileOpError::too_large("file write exceeds the byte limit"));
    }
    if offset
        .checked_add(data.len() as u64)
        .is_none_or(|end| end > i64::MAX as u64)
    {
        return Err(FileOpError::invalid_request(
            "file write range exceeds supported offsets",
        ));
    }
    if path.is_empty() || path.contains('\0') {
        return Err(FileOpError::invalid_path(
            "file write needs a nonempty path without NUL bytes",
        ));
    }
    if mode.is_some_and(|mode| mode > 0o7777) {
        return Err(FileOpError::invalid_request("file mode contains non-permission bits"));
    }
    let (parent, name) = split_parent(path);
    if name.is_empty() {
        return Err(FileOpError::failed(
            FileOpErrorKind::IsADirectory,
            "file write needs a file path",
        ));
    }
    match guest.stat(path).await {
        Ok(metadata) if metadata.kind == FileKind::Directory => {
            return Err(FileOpError::failed(
                FileOpErrorKind::IsADirectory,
                format!("{path} is a directory"),
            ));
        }
        Ok(_) => {}
        Err(FileOpError::NotFound { .. }) if create_parents.creates() => {}
        Err(FileOpError::NotFound { .. }) => match guest.stat(parent).await {
            Ok(metadata) if matches!(metadata.kind, FileKind::Directory | FileKind::Symlink) => {}
            Ok(_) => {
                return Err(FileOpError::failed(
                    FileOpErrorKind::NotADirectory,
                    format!("{parent} is not a directory"),
                ));
            }
            Err(FileOpError::NotFound { .. }) => {
                return Err(FileOpError::NotFound { path: (*path).into() });
            }
            Err(error) => return Err(error),
        },
        Err(error) => return Err(error),
    }
    let temporary = format!("{parent}/.trogon-write-{}", Ulid::generate());
    let size = data.len() as u64;
    let staged = guest
        .envd
        .write_file(guest.target, guest.user, &temporary, data)
        .await
        .map_err(|error| api_failure(error, path));
    let outcome = match staged {
        Ok(()) => {
            let args = vec![
                (*path).to_string(),
                temporary.clone(),
                offset.to_string(),
                u8::from(truncate.truncates()).to_string(),
                mode.map(|mode| format!("{mode:o}")).unwrap_or_default(),
            ];
            guest.script(WRITE_SCRIPT, args, SCRIPT_TIMEOUT, path).await
        }
        Err(error) => Err(error),
    };
    guest.discard_logged(&temporary).await;
    let output = outcome?;
    if output.exit_code != 0 {
        return Err(script_failure("file write", &output));
    }
    let metadata = guest.stat(path).await?;
    Ok(FileOpResponse::Written {
        bytes: size,
        size: metadata.size,
    })
}
