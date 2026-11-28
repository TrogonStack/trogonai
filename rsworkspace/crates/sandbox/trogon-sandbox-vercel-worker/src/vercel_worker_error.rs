use async_nats::{PublishError, SubscribeError};
use trogon_sandbox_contract::{LeaseError, TokenError};
use trogon_sandbox_observability::ObservabilityError;
use trogon_sandbox_protocol::{AuthorityModeError, NatsConnectError, NatsTransferError};

use crate::{VercelApiError, VercelDispatchError, VercelProviderError};

/// The worker could not start, or one request it served failed.
#[derive(Debug, thiserror::Error)]
pub enum VercelWorkerError {
    #[error("subscribing failed: {0}")]
    Subscribe(#[from] SubscribeError),
    #[error("publishing failed: {0}")]
    Publish(#[from] PublishError),
    #[error("a message could not be encoded: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("a file transfer failed: {0}")]
    Transfer(#[from] NatsTransferError),
    #[error("worker authority is misconfigured: {0}")]
    Authority(#[from] AuthorityModeError),
    #[error("the worker id is invalid: {0}")]
    WorkerId(#[from] TokenError),
    #[error(transparent)]
    Provider(#[from] VercelProviderError),
    #[error("connecting to NATS failed: {0}")]
    Connect(#[from] NatsConnectError),
    #[error("SANDBOX_VERCEL_RUNTIME: {0}")]
    Runtime(#[from] LeaseError),
    #[error(transparent)]
    Api(#[from] VercelApiError),
    #[error(transparent)]
    Observability(#[from] ObservabilityError),
    #[error(transparent)]
    Dispatch(#[from] VercelDispatchError),
    #[error("the dispatch ledger could not be opened: {0}")]
    Ledger(#[from] trogon_sandbox_protocol::AttemptLedgerError),
}
