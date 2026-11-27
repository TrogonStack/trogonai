use async_nats::SubscribeError;
use tokio::task::JoinError;
use trogon_sandbox_contract::WorkerId;
use trogon_sandbox_protocol::AuthorityModeError;

/// Why the worker loop could not start or stop cleanly.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// The worker was started under an identity other than the one its provider owns leases as.
    #[error("worker ID {worker_id} differs from its provider ownership identity {owner}")]
    WorkerIdMismatch { worker_id: WorkerId, owner: WorkerId },
    /// The authority mode named in the environment is not valid.
    #[error(transparent)]
    Authority(#[from] AuthorityModeError),
    /// A subscription could not be created.
    #[error("failed to subscribe: {0}")]
    Subscribe(#[from] SubscribeError),
    /// The authority guard task ended abnormally.
    #[error("authority guard task failed: {0}")]
    AuthorityTask(#[from] JoinError),
}
