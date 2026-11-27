use trogon_sandbox_protocol::AttemptLedgerError;

/// Why a worker's state directory could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum SpaceProviderOpenError {
    /// A file operation on the state directory failed.
    #[error("state directory is unusable: {0}")]
    Io(#[from] std::io::Error),
    /// Another worker holds the state directory.
    #[error("another worker owns this state directory: {0}")]
    Locked(#[from] std::fs::TryLockError),
    /// The dispatch ledger could not be opened.
    #[error(transparent)]
    Ledger(#[from] AttemptLedgerError),
}
