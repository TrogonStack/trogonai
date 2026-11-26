use std::io;

use trogon_sandbox_contract::{Base64DataError, FileOpErrorKind, FileOpResponse, FileSha256Error};

/// Why a file operation produced no result, in the terms the wire reply uses.
///
/// Converting a failure into a [`FileOpResponse`] yields the typed reply a worker publishes.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum FileOpFailure {
    /// The path does not exist.
    #[error("{path}: not found")]
    NotFound { path: String },
    /// The operation failed for the reason `kind` names.
    #[error("{kind:?}: {message}")]
    Failed { kind: FileOpErrorKind, message: String },
    /// The worker does not serve this operation.
    #[error("{message}")]
    Unsupported { message: String },
}

impl FileOpFailure {
    pub fn failed(kind: FileOpErrorKind, message: impl Into<String>) -> Self {
        Self::Failed {
            kind,
            message: message.into(),
        }
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::failed(FileOpErrorKind::TooLarge, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::failed(FileOpErrorKind::InvalidRequest, message)
    }

    pub fn invalid_path(message: impl Into<String>) -> Self {
        Self::failed(FileOpErrorKind::InvalidPath, message)
    }

    /// Maps an I/O error raised while serving `path` onto the failure a caller can act on.
    pub fn from_io(error: io::Error, path: &str) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::NotFound => {
                return Self::NotFound { path: path.to_string() };
            }
            io::ErrorKind::PermissionDenied => FileOpErrorKind::PermissionDenied,
            io::ErrorKind::AlreadyExists => FileOpErrorKind::AlreadyExists,
            io::ErrorKind::NotADirectory => FileOpErrorKind::NotADirectory,
            io::ErrorKind::IsADirectory => FileOpErrorKind::IsADirectory,
            io::ErrorKind::DirectoryNotEmpty => FileOpErrorKind::DirectoryNotEmpty,
            io::ErrorKind::InvalidInput => FileOpErrorKind::InvalidPath,
            _ => FileOpErrorKind::Io,
        };
        Self::failed(kind, format!("{path}: {error}"))
    }
}

impl From<FileOpFailure> for FileOpResponse {
    fn from(failure: FileOpFailure) -> Self {
        match failure {
            FileOpFailure::NotFound { path } => Self::NotFound { path },
            FileOpFailure::Failed { kind, message } => Self::Failed { kind, message },
            FileOpFailure::Unsupported { message } => Self::Unsupported { message },
        }
    }
}

impl From<FileSha256Error> for FileOpFailure {
    fn from(error: FileSha256Error) -> Self {
        Self::failed(FileOpErrorKind::Io, format!("digest could not be encoded: {error}"))
    }
}

impl From<Base64DataError> for FileOpFailure {
    fn from(error: Base64DataError) -> Self {
        Self::failed(FileOpErrorKind::Io, format!("payload could not be encoded: {error}"))
    }
}
