use crate::{
    MaxLeasesError, SpaceApiError, SpaceConfigError, SpaceOriginError, SpaceProviderError, SpaceProviderOpenError,
    SpaceTemplateError, SpaceTokenError, SpaceWorkerError,
};

/// Why the worker could not start or stopped abnormally.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error(transparent)]
    Origin(#[from] SpaceOriginError),
    #[error(transparent)]
    Token(#[from] SpaceTokenError),
    #[error(transparent)]
    Template(#[from] SpaceTemplateError),
    #[error(transparent)]
    MaxLeases(#[from] MaxLeasesError),
    #[error(transparent)]
    Config(#[from] SpaceConfigError),
    #[error(transparent)]
    Api(#[from] SpaceApiError),
    #[error(transparent)]
    Authority(#[from] trogon_sandbox_protocol::AuthorityModeError),
    #[error("cannot open the worker state: {0}")]
    Open(#[from] SpaceProviderOpenError),
    #[error("cannot reclaim sandboxes from an earlier run: {0}")]
    Recover(#[from] SpaceProviderError),
    #[error("configured worker ID differs from its durable identity")]
    WorkerIdMismatch,
    #[error(transparent)]
    Connect(#[from] trogon_sandbox_protocol::NatsConnectError),
    #[error(transparent)]
    Worker(#[from] SpaceWorkerError),
}
