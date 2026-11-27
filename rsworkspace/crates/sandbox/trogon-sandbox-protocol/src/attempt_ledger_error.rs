use trogon_sandbox_contract::TokenError;

/// Why the durable attempt ledger refused or failed an operation.
#[derive(Debug, thiserror::Error)]
pub enum AttemptLedgerError {
    /// A file operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The database failed.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// A stored value is not valid JSON of its type.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The ledger path has no parent directory.
    #[error("ledger has no parent")]
    NoParent,
    /// Another process holds the ledger's lock file.
    #[error("another worker owns the dispatch ledger")]
    Locked,
    /// The legacy journal is too large to migrate.
    #[error("legacy dispatch journal exceeds migration budget")]
    LegacyTooLarge,
    /// The legacy journal is not readable.
    #[error("invalid durable dispatch ledger: {0}")]
    LegacyInvalid(serde_json::Error),
    /// The legacy journal belongs to another version, provider or worker.
    #[error("legacy ledger identity mismatch")]
    LegacyIdentityMismatch,
    /// A legacy attempt does not belong to its journal.
    #[error("legacy attempt identity mismatch")]
    LegacyAttemptMismatch,
    /// The database is of another format.
    #[error("dispatch database format mismatch")]
    FormatMismatch,
    /// The database belongs to another provider.
    #[error("dispatch database provider mismatch")]
    ProviderMismatch,
    /// The database holds attempts but no metadata.
    #[error("dispatch database metadata is missing")]
    MetadataMissing,
    /// The requested worker identity differs from the one the database holds.
    #[error("worker identity differs from its durable ledger")]
    WorkerIdentityMismatch,
    /// The stored worker identity is invalid.
    #[error("invalid durable worker identity: {0}")]
    InvalidWorkerId(TokenError),
    /// A stored attempt identity is invalid.
    #[error("invalid durable attempt identity: {0}")]
    InvalidAttemptId(TokenError),
    /// A stored attempt phase is unknown.
    #[error("invalid attempt phase")]
    InvalidPhase,
    /// A stored cleanup intent is unknown.
    #[error("invalid cleanup intent")]
    InvalidIntent,
    /// A stored assignment epoch is invalid.
    #[error("invalid durable assignment epoch")]
    InvalidEpoch,
    /// A recovered attempt vanished during recovery.
    #[error("recovered attempt disappeared")]
    RecoveredAttemptDisappeared,
    /// The backend receipt is larger than the ledger stores.
    #[error("backend receipt exceeds its storage budget")]
    ReceiptTooLarge,
    /// An attempt id is already used by another identity.
    #[error("dispatch attempt identity was reused")]
    AttemptIdentityReused,
    /// The database path no longer resolves.
    #[error("dispatch database path is unavailable: {0}")]
    PathUnavailable(std::io::Error),
    /// The database path is not a regular file.
    #[error("dispatch database path is not a file")]
    PathNotFile,
    /// The database file was replaced after it was opened.
    #[error("dispatch database path was replaced")]
    PathReplaced,
    /// The identity names another worker or provider.
    #[error("dispatch attempt targets a different worker or provider")]
    ForeignIdentity,
    /// The identity names no lease.
    #[error("lease identity is empty")]
    EmptyLease,
    /// A receipt arrived for an attempt that never started.
    #[error("backend receipt has no started attempt")]
    ReceiptWithoutAttempt,
    /// A receipt arrived for another identity or for an attempt past allocation.
    #[error("backend receipt identity or phase mismatch")]
    ReceiptMismatch,
    /// The backend resource of an attempt changed.
    #[error("attempt backend identity changed")]
    BackendIdentityChanged,
    /// A ready attempt holds no receipt.
    #[error("ready attempt has no receipt")]
    ReadyWithoutReceipt,
    /// The attempt started earlier and needs cleanup or recovery.
    #[error("attempt already started and requires cleanup or recovery")]
    AlreadyStarted,
    /// The attempt was cancelled.
    #[error("attempt was durably cancelled")]
    Cancelled,
    /// The attempt vanished while its allocation ran.
    #[error("attempt disappeared")]
    AttemptDisappeared,
    /// A cancel arrived while the allocation was in flight.
    #[error("attempt was cancelled while allocation was in flight")]
    CancelledDuringAllocation,
    /// A cleanup names an attempt id that another identity uses.
    #[error("cleanup attempt identity was reused")]
    CleanupIdentityReused,
    /// The cleanup attempt vanished.
    #[error("cleanup attempt disappeared")]
    CleanupAttemptDisappeared,
    /// A purge arrived while a release teardown was in flight.
    #[error("cleanup intent escalated while backend teardown was in flight")]
    IntentEscalated,
    /// The backend allocation failed.
    #[error("{0}")]
    Allocation(Box<dyn std::error::Error + Send + Sync>),
    /// The backend teardown failed.
    #[error("{0}")]
    Teardown(Box<dyn std::error::Error + Send + Sync>),
}
