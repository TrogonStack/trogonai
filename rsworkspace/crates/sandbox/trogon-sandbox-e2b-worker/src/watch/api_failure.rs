use trogon_sandbox_contract::FileOpErrorKind;
use trogon_sandbox_file_ops::FileOpError;

use crate::api::ApiError;

/// The file operation failure an envd call error stands for while serving `path`.
pub(crate) fn api_failure(error: &ApiError, path: &str) -> FileOpError {
    match error {
        ApiError::NotFound(_) => FileOpError::NotFound { path: path.to_owned() },
        ApiError::Unauthorized(message) => FileOpError::failed(FileOpErrorKind::PermissionDenied, message.clone()),
        ApiError::Rejected { message, .. } => FileOpError::invalid_request(message.clone()),
        other => FileOpError::failed(FileOpErrorKind::Io, other.to_string()),
    }
}
