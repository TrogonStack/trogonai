use trogon_sandbox_contract::FileOpErrorKind;
use trogon_sandbox_file_ops::FileOpError;

use crate::api::ApiError;

/// The file operation failure an envd call error stands for while serving `path`.
pub(super) fn api_failure(error: ApiError, path: &str) -> FileOpError {
    match error {
        ApiError::NotFound(_) => FileOpError::NotFound { path: path.into() },
        ApiError::Unauthorized(message) => FileOpError::failed(FileOpErrorKind::PermissionDenied, message),
        error => FileOpError::failed(FileOpErrorKind::Io, error.to_string()),
    }
}
