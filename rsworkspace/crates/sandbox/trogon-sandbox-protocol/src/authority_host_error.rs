use crate::AttemptLedgerError;

/// Why a host could not report, freeze, thaw or purge a lease.
#[derive(Debug, thiserror::Error)]
pub enum AuthorityHostError {
    /// The host failed.
    #[error("{0}")]
    Failed(Box<dyn std::error::Error + Send + Sync>),
    /// The provider reported that cleanup failed.
    #[error("{message}")]
    CleanupFailed {
        /// What the provider reported.
        message: String,
    },
    /// The cleanup succeeded but the ledger does not record it.
    #[error("exact backend cleanup lacks durable confirmation")]
    ConfirmationMissing,
    /// The cleanup reply names another assignment or intent.
    #[error("cleanup reply does not confirm the exact assignment and purge intent")]
    CleanupReplyMismatch,
    /// The attempt ledger failed.
    #[error(transparent)]
    Ledger(#[from] AttemptLedgerError),
}
