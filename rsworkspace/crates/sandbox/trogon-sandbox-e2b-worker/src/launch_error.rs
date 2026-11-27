use trogon_sandbox_protocol::{AttemptLedgerError, AuthorityModeError, NatsConnectError};

use crate::api::{ApiError, E2bConfigError};
use crate::{E2bDispatchError, E2bWorkerError};

/// Why the worker could not start or stopped abnormally.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// `SANDBOX_WORKER_AUTHORITY` is not a mode.
    #[error(transparent)]
    Authority(#[from] AuthorityModeError),
    /// The E2B endpoint, domain or key is not usable.
    #[error(transparent)]
    Config(#[from] E2bConfigError),
    /// An HTTP client could not be built.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The worker identity is not a valid token.
    #[error("invalid worker id: {0}")]
    WorkerId(#[from] trogon_sandbox_contract::TokenError),
    /// The dispatch ledger could not be opened.
    #[error(transparent)]
    Ledger(#[from] AttemptLedgerError),
    /// The dispatch ledger could not be attached to the provider.
    #[error(transparent)]
    Dispatch(#[from] E2bDispatchError),
    /// NATS is unreachable.
    #[error(transparent)]
    Connect(#[from] NatsConnectError),
    /// The worker failed while serving.
    #[error(transparent)]
    Worker(#[from] E2bWorkerError),
}
