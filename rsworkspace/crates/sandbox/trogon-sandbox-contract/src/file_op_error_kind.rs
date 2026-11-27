use serde::{Deserialize, Serialize};

/// Why a file operation failed.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOpErrorKind {
    /// The path escapes the sandbox filesystem the worker exposes, or is malformed.
    InvalidPath,
    PermissionDenied,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    /// The request or its result exceeds a size limit, and the message names it.
    TooLarge,
    /// The request itself is malformed, such as a bad regex, an empty `find`, or a bad archive.
    InvalidRequest,
    Io,
    /// The `since` of a changes request names a checkpoint this lease does not hold: never
    /// created, already evicted, or created on another lease.
    UnknownCheckpoint,
    /// The watch id is not live on this lease: stopped, expired from disuse, or never started.
    UnknownWatch,
}
