use trogon_sandbox_contract::{
    FileOpErrorKind, FileOpResponse, FileOverwrite, FileParentCreation, FilePresence, FileRecursion,
};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::constants::{
    EXIT_EXISTS, EXIT_MISSING, EXIT_NO_TOOL, EXIT_NOOP, EXIT_ROOT, MKDIR, REMOVE, RENAME_NO_REPLACE, SCRIPT_TIMEOUT,
};
use super::file_guest::FileGuest;
use super::guest_bodies::MoveBody;
use super::script_failure::stderr_excerpt;
use super::stderr_kind::stderr_kind;
use crate::envd::ProcessResult;

/// Refuses an empty or NUL path, and for a destructive operation also an absolute path that resolves to `/`.
pub(super) fn validate(path: &str, destructive: bool) -> Result<(), FileOpError> {
    if path.is_empty() || path.contains('\0') {
        return Err(FileOpError::invalid_path("path is empty or contains NUL"));
    }
    if destructive && path.starts_with('/') {
        let mut components = Vec::new();
        for component in path.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    components.pop();
                }
                component => components.push(component),
            }
        }
        if components.is_empty() {
            return Err(FileOpError::invalid_path("cannot mutate the sandbox root"));
        }
    }
    Ok(())
}

/// The failure a path script that exited nonzero stands for.
pub(super) fn failure(output: &ProcessResult, path: &str) -> FileOpError {
    match output.exit_code {
        EXIT_MISSING => return FileOpError::NotFound { path: path.into() },
        EXIT_ROOT => return FileOpError::invalid_path("cannot mutate the sandbox root"),
        EXIT_EXISTS => {
            return FileOpError::failed(FileOpErrorKind::AlreadyExists, format!("{path} already exists"));
        }
        EXIT_NO_TOOL => {
            return FileOpError::Unsupported {
                message: "image is missing a required filesystem tool".into(),
            };
        }
        _ => {}
    }
    let message = stderr_excerpt(output);
    if message
        .rsplit(": ")
        .next()
        .unwrap_or(&message)
        .to_lowercase()
        .contains("no such file")
    {
        return FileOpError::NotFound { path: path.into() };
    }
    FileOpError::failed(
        stderr_kind(&message),
        format!("filesystem operation failed (exit {}): {message}", output.exit_code),
    )
}

pub(super) async fn mkdir(
    guest: &FileGuest<'_>,
    path: &str,
    parents: FileParentCreation,
) -> Result<FileOpResponse, FileOpError> {
    validate(path, false)?;
    let args = vec![path.to_string(), u8::from(parents.creates()).to_string()];
    let output = guest
        .script(MKDIR, args, SCRIPT_TIMEOUT, "filesystem operation")
        .await?;
    match output.exit_code {
        0 => Ok(FileOpResponse::Created {
            already_existed: FilePresence::Absent,
        }),
        EXIT_NOOP => Ok(FileOpResponse::Created {
            already_existed: FilePresence::Present,
        }),
        _ => Err(failure(&output, path)),
    }
}

pub(super) async fn remove(
    guest: &FileGuest<'_>,
    path: &str,
    recursion: FileRecursion,
) -> Result<FileOpResponse, FileOpError> {
    validate(path, true)?;
    let args = vec![path.to_string(), u8::from(recursion.is_recursive()).to_string()];
    let output = guest
        .script(REMOVE, args, SCRIPT_TIMEOUT, "filesystem operation")
        .await?;
    match output.exit_code {
        0 => Ok(FileOpResponse::Removed {
            existed: FilePresence::Present,
        }),
        EXIT_NOOP => Ok(FileOpResponse::Removed {
            existed: FilePresence::Absent,
        }),
        _ => match failure(&output, path) {
            FileOpError::NotFound { .. } => Ok(FileOpResponse::Removed {
                existed: FilePresence::Absent,
            }),
            error => Err(error),
        },
    }
}

pub(super) async fn rename(
    guest: &FileGuest<'_>,
    from: &str,
    to: &str,
    overwrite: FileOverwrite,
) -> Result<FileOpResponse, FileOpError> {
    validate(from, true)?;
    validate(to, true)?;
    if overwrite.overwrites() {
        guest
            .rpc(
                "Move",
                MoveBody {
                    source: from,
                    destination: to,
                },
            )
            .await
            .map_err(|error| api_failure(error, from))?;
        return Ok(FileOpResponse::Renamed);
    }
    let args = vec![from.to_string(), to.to_string()];
    let output = guest
        .script(RENAME_NO_REPLACE, args, SCRIPT_TIMEOUT, "filesystem operation")
        .await?;
    match output.exit_code {
        0 => Ok(FileOpResponse::Renamed),
        EXIT_MISSING => Err(failure(&output, from)),
        _ => Err(failure(&output, to)),
    }
}
