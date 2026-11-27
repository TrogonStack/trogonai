use trogon_sandbox_contract::{ReapLeasesRequestError, SandboxTagsError};

/// Why a request payload was refused.
#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error("{0}")]
    Malformed(#[from] serde_json::Error),
    #[error("invalid tags: {0}")]
    Tags(#[from] SandboxTagsError),
    #[error("{0}")]
    ReapFilter(#[from] ReapLeasesRequestError),
    #[error("bucket and provider_volume are mutually exclusive")]
    AmbiguousDriveStorage,
    #[error("missing bucket or provider_volume")]
    MissingDriveStorage,
    #[error("give entity_id or name, not both")]
    AmbiguousEntity,
    #[error("missing entity_id or name")]
    MissingEntity,
    #[error("operation capabilities takes an empty session-scoped payload")]
    NonEmptyCapabilitiesPayload,
    #[error("operation lookup takes only operation_id")]
    CompensateOnLookup,
}
