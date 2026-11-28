use trogon_sandbox_protocol::AttemptLedgerError;

use crate::{VercelApiError, VercelProviderError};

/// Why a targeted dispatch or cleanup could not be served.
#[derive(Debug, thiserror::Error)]
pub enum VercelDispatchError {
    #[error("this worker keeps no dispatch ledger, so it takes no targeted dispatch")]
    NotConfigured,
    #[error("the worker already keeps a dispatch ledger")]
    AlreadyConfigured,
    #[error("a targeted dispatch must name its attempt")]
    Untargeted,
    #[error("the dispatch is addressed to another worker or provider")]
    TargetMismatch,
    #[error("the attempt has no receipt, so the sandbox to delete is unknown")]
    CleanupWithoutReceipt,
    #[error("this worker holds another sandbox for the lease")]
    ForeignSandbox,
    #[error(transparent)]
    Ledger(#[from] AttemptLedgerError),
    #[error(transparent)]
    Provider(#[from] VercelProviderError),
    #[error(transparent)]
    Api(#[from] VercelApiError),
}
