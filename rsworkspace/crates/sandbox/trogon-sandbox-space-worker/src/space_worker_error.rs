use crate::SpaceProviderError;

/// Why the worker loop ended abnormally.
#[derive(Debug, thiserror::Error)]
pub enum SpaceWorkerError {
    /// A subscription, flush or unsubscribe on the NATS connection failed.
    #[error("NATS request failed: {0}")]
    Nats(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// Reclaiming owned sandboxes at shutdown failed.
    #[error("shutdown reconciliation failed: {0}")]
    Shutdown(#[from] SpaceProviderError),
}
