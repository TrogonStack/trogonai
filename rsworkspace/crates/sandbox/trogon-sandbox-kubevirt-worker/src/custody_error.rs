use crate::{BoundedNameError, CustodyTokenError};

/// The durable custody journal refused or failed an operation.
#[derive(Debug, thiserror::Error)]
pub enum CustodyError {
    #[error("custody storage failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("a custody file is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the custody namespace is invalid: {0}")]
    Namespace(#[from] BoundedNameError),
    #[error("a custody file is not a regular file within its size budget")]
    InvalidFile,
    #[error("a custody record exceeds its size budget")]
    RecordTooLarge,
    #[error("the custody state path is not a directory")]
    NotDirectory,
    #[error("the custody directory belongs to another worker or namespace")]
    ForeignOwner,
    #[error("the custody directory is already held by a worker")]
    AlreadyHeld,
    #[error("the custody directory exceeds its budget")]
    DirectoryBudget,
    #[error("the custody directory holds unrecognized content")]
    UnrecognizedContent,
    #[error("a custody file name is not valid UTF-8")]
    InvalidFileName,
    #[error("the custody lease identity is invalid")]
    InvalidLeaseIdentity,
    #[error("a custody record is unsupported or belongs to another worker")]
    ForeignRecord,
    #[error("a custody record names a different lease than its file")]
    LeaseMismatch,
    #[error("a settled custody record still retains resources")]
    SettledWithResources,
    #[error("a ready custody record lacks a complete resource identity")]
    IncompleteReady,
    #[error("a custody resource identity is invalid")]
    InvalidResource,
    #[error("a warm claim proof is invalid")]
    InvalidClaim,
    #[error("a warm claim scope differs from the durable ownership")]
    ClaimScope,
    #[error("a custody token is invalid: {0}")]
    Token(#[from] CustodyTokenError),
    #[error("an unresolved create intent is invalid")]
    InvalidCreateIntent,
    #[error("durable custody must settle before another allocation")]
    NotSettled,
    #[error("custody capacity is exhausted")]
    CapacityExhausted,
    #[error("the exact custody record is unavailable")]
    RecordUnavailable,
    #[error("the custody allocation generation changed")]
    GenerationChanged,
    #[error("the prior create outcome is unresolved")]
    CreateUnresolved,
    #[error("exact custody cleanup is not settled")]
    CleanupUnsettled,
}
