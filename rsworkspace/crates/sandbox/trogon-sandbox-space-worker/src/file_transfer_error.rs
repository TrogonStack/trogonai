use crate::{ExecFailureError, SpaceProviderError};

/// Why moving a file through the guest failed.
#[derive(Debug, thiserror::Error)]
pub enum FileTransferError {
    /// The lease is unknown to this worker.
    #[error(transparent)]
    Lease(#[from] SpaceProviderError),
    /// The request names a user to act as.
    #[error("as_user is unavailable through this worker")]
    AsUser,
    /// The write is too large or has too many files.
    #[error("file write exceeds 16 MiB or file count limit")]
    TooMuchData,
    /// The path is empty or holds a NUL byte.
    #[error("invalid file path")]
    InvalidPath,
    /// A relative path has no lease directory to resolve against.
    #[error("a relative path needs the lease to set a working directory")]
    RelativePath,
    /// The mode has bits beyond the permission and special bits.
    #[error("invalid Unix file mode")]
    InvalidMode,
    /// A guest command exited non-zero.
    #[error("guest file operation failed")]
    GuestFailed,
    /// The guest did not name its temporary file.
    #[error("guest created no temporary file")]
    NoTemporaryFile,
    /// A guest command hit its deadline.
    #[error("guest file operation timed out")]
    GuestTimedOut,
    /// The guest could not read the file.
    #[error("file read failed")]
    ReadFailed,
    /// The guest's reply is not valid base64.
    #[error("guest returned file data that was not base64")]
    NotBase64,
    /// The file is larger than the worker moves.
    #[error("file exceeds 16 MiB limit")]
    TooLarge,
    /// Reading took longer than the worker allows.
    #[error("file read timed out")]
    ReadTimedOut,
    /// A guest command could not be run.
    #[error(transparent)]
    Exec(#[from] ExecFailureError),
}
